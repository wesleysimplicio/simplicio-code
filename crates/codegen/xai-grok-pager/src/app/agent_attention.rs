//! Passive projection of the independently shipped Simplicio Agent host.
//!
//! The pager polls only the existing `host.status` and `host.advisories`
//! operations. It never turns an advisory into an effect: this state is a
//! read-only input to the side panel.

use simplicio_agent_client::{AgentAttentionState, AgentHostClient, Error};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Healthy poll cadence owned by the pager's existing event-loop timer set.
pub const HEALTHY_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Failed polls back off exponentially but never leave the panel stale longer
/// than this cap before retrying the host.
pub const MAX_POLL_BACKOFF: Duration = Duration::from_secs(60);

/// Cancellation owner for the pager's Agent polling tasks. The drop guard
/// makes early returns equivalent to the explicit shutdown path.
pub struct AgentAttentionPollLifecycle {
    token: CancellationToken,
    _drop_guard: tokio_util::sync::DropGuard,
}

impl Default for AgentAttentionPollLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentAttentionPollLifecycle {
    pub fn new() -> Self {
        let token = CancellationToken::new();
        let drop_guard = token.clone().drop_guard();
        Self {
            token,
            _drop_guard: drop_guard,
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    pub fn cancel(&self) {
        self.token.cancel();
    }
}

/// Result of one bounded status + advisory poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAttentionPollResult {
    Ready {
        profile: AgentHostProfile,
        attention: AgentAttentionState,
    },
    Degraded(AgentHostDegradedReason),
    Cancelled,
}

/// Safe, bounded display projection of the host's untrusted profile string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHostProfile {
    Desktop,
    Compatible,
}

impl AgentHostProfile {
    pub(crate) fn from_untrusted(profile: &str) -> Self {
        match profile {
            "desktop" => Self::Desktop,
            _ => Self::Compatible,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Compatible => "compatible",
        }
    }
}

/// Fixed UI-safe catalog for failures. Client error strings may contain local
/// socket paths or hostile response details and must never enter app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentHostDegradedReason {
    AgentUnavailable,
    SocketRejected,
    UnsupportedTransport,
    TransportIo,
    InvalidResponse,
    HostNotReady,
    ProtocolMismatch,
    CapabilityMismatch,
    InvalidAdvisoryCursor,
    PollTaskFailed,
}

impl AgentHostDegradedReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::AgentUnavailable => "agent_unavailable",
            Self::SocketRejected => "socket_rejected",
            Self::UnsupportedTransport => "unsupported_transport",
            Self::TransportIo => "transport_io",
            Self::InvalidResponse => "invalid_response",
            Self::HostNotReady => "host_not_ready",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::CapabilityMismatch => "capability_mismatch",
            Self::InvalidAdvisoryCursor => "invalid_advisory_cursor",
            Self::PollTaskFailed => "poll_task_failed",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::AgentUnavailable => "Agent host is unavailable.",
            Self::SocketRejected => "Agent socket was rejected.",
            Self::UnsupportedTransport => "Agent transport is unsupported.",
            Self::TransportIo => "Agent transport failed.",
            Self::InvalidResponse => "Agent response was invalid.",
            Self::HostNotReady => "Agent host is not ready.",
            Self::ProtocolMismatch => "Agent protocol is incompatible.",
            Self::CapabilityMismatch => "Agent capabilities are incomplete.",
            Self::InvalidAdvisoryCursor => "Agent advisory cursor was rejected.",
            Self::PollTaskFailed => "Agent status poll failed.",
        }
    }
}

/// Operational status shown by the passive panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentHostStatus {
    Connecting,
    Ready { profile: AgentHostProfile },
    Degraded { reason: AgentHostDegradedReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttentionMergeError {
    InvalidCursor,
}

/// App-owned state for the non-focusable Agent panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttentionPanelState {
    pub status: AgentHostStatus,
    pub attention: Option<AgentAttentionState>,
    poll_in_flight: bool,
    consecutive_failures: u8,
}

impl Default for AgentAttentionPanelState {
    fn default() -> Self {
        Self {
            status: AgentHostStatus::Connecting,
            attention: None,
            poll_in_flight: false,
            consecutive_failures: 0,
        }
    }
}

impl AgentAttentionPanelState {
    /// Claim the single poll slot and return the advisory cursor to request.
    /// `None` means another poll is already in flight.
    pub fn begin_poll(&mut self) -> Option<u64> {
        if self.poll_in_flight {
            return None;
        }
        self.poll_in_flight = true;
        Some(self.attention.as_ref().map_or(0, |state| state.cursor))
    }

    /// Healthy cadence with capped exponential backoff after failures.
    pub fn next_poll_delay(&self) -> Duration {
        let multiplier = 1u32 << self.consecutive_failures.min(4);
        HEALTHY_POLL_INTERVAL
            .saturating_mul(multiplier)
            .min(MAX_POLL_BACKOFF)
    }

    pub fn complete_poll(&mut self, result: AgentAttentionPollResult) {
        self.poll_in_flight = false;
        match result {
            AgentAttentionPollResult::Ready { profile, attention } => {
                match merge_attention(&mut self.attention, attention) {
                    Ok(()) => {
                        self.consecutive_failures = 0;
                        self.status = AgentHostStatus::Ready { profile };
                    }
                    Err(AttentionMergeError::InvalidCursor) => {
                        self.record_failure(AgentHostDegradedReason::InvalidAdvisoryCursor);
                    }
                }
            }
            AgentAttentionPollResult::Degraded(reason) => {
                self.record_failure(reason);
            }
            AgentAttentionPollResult::Cancelled => {}
        }
    }

    fn record_failure(&mut self, reason: AgentHostDegradedReason) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = AgentHostStatus::Degraded { reason };
    }

    #[cfg(test)]
    fn poll_in_flight(&self) -> bool {
        self.poll_in_flight
    }
}

/// Run one host poll off the event-loop thread.
///
/// Each transport request is already bounded by
/// `simplicio_agent_client::DEFAULT_REQUEST_TIMEOUT_MS` and the client's
/// response-size cap. Cancellation is checked before status, between status
/// and advisories, and after advisories; an in-progress blocking socket read
/// therefore finishes no later than its transport timeout.
pub fn poll_agent_attention(cursor: u64, cancel: &CancellationToken) -> AgentAttentionPollResult {
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled;
    }
    let client = match AgentHostClient::connect_default() {
        Ok(client) => client,
        Err(error) => return AgentAttentionPollResult::Degraded(safe_reason(&error)),
    };
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled;
    }
    let profile = AgentHostProfile::from_untrusted(&client.capabilities().profile);
    let page = match client.advisories(cursor) {
        Ok(page) => page,
        Err(error) => return AgentAttentionPollResult::Degraded(safe_reason(&error)),
    };
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled;
    }
    AgentAttentionPollResult::Ready {
        profile,
        attention: page.attention_state(),
    }
}

fn merge_attention(
    current: &mut Option<AgentAttentionState>,
    incoming: AgentAttentionState,
) -> Result<(), AttentionMergeError> {
    let requested_cursor = current.as_ref().map_or(0, |state| state.cursor);
    let advanced_by = incoming
        .cursor
        .checked_sub(requested_cursor)
        .ok_or(AttentionMergeError::InvalidCursor)?;
    let event_count =
        u64::try_from(incoming.unread).map_err(|_| AttentionMergeError::InvalidCursor)?;
    // An empty validated page keeps its requested cursor. A non-empty page
    // must advance at least once per strictly increasing event sequence.
    if (event_count == 0 && advanced_by != 0)
        || (event_count > 0 && (advanced_by == 0 || event_count > advanced_by))
    {
        return Err(AttentionMergeError::InvalidCursor);
    }
    if current.is_none() {
        *current = Some(incoming);
        return Ok(());
    }
    let current = current
        .as_mut()
        .expect("Agent attention was initialized above");
    // The client validates every page against the requested cursor, and the
    // state machine is single-flight. Keep this local guard as defense in
    // depth against a stale/out-of-order task result ever reaching the model.
    current.cursor = incoming.cursor;
    current.unread = current.unread.saturating_add(incoming.unread);
    current.highest_severity = match (current.highest_severity, incoming.highest_severity) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (severity @ Some(_), None) | (None, severity @ Some(_)) => severity,
        (None, None) => None,
    };
    if incoming.latest_summary.is_some() {
        current.latest_summary = incoming.latest_summary;
        // The action belongs to the latest advisory. Clear an older action
        // when the new latest advisory intentionally has none.
        current.suggested_action = incoming.suggested_action;
    }
    current.history_truncated |= incoming.history_truncated;
    Ok(())
}

pub(crate) fn safe_reason(error: &Error) -> AgentHostDegradedReason {
    match error {
        Error::AgentNotFound(_) => AgentHostDegradedReason::AgentUnavailable,
        Error::InsecureSocket(_) => AgentHostDegradedReason::SocketRejected,
        Error::UnsupportedTransport => AgentHostDegradedReason::UnsupportedTransport,
        Error::Io(_) => AgentHostDegradedReason::TransportIo,
        Error::InvalidResponse(_) => AgentHostDegradedReason::InvalidResponse,
        Error::OperationRejected(_) => AgentHostDegradedReason::HostNotReady,
        Error::ProtocolMismatch(_) => AgentHostDegradedReason::ProtocolMismatch,
        Error::CapabilityMismatch { .. } => AgentHostDegradedReason::CapabilityMismatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simplicio_agent_client::AdvisorySeverity;

    fn attention(
        cursor: u64,
        unread: usize,
        severity: Option<AdvisorySeverity>,
        summary: Option<&str>,
        action: Option<&str>,
    ) -> AgentAttentionState {
        AgentAttentionState {
            cursor,
            unread,
            highest_severity: severity,
            latest_summary: summary.map(str::to_owned),
            suggested_action: action.map(str::to_owned),
            history_truncated: false,
        }
    }

    #[test]
    fn admits_only_one_poll_at_a_time() {
        let mut state = AgentAttentionPanelState::default();
        assert_eq!(state.begin_poll(), Some(0));
        assert_eq!(state.begin_poll(), None);
        assert!(state.poll_in_flight());

        state.complete_poll(AgentAttentionPollResult::Degraded(
            AgentHostDegradedReason::AgentUnavailable,
        ));
        assert!(!state.poll_in_flight());
        assert_eq!(state.begin_poll(), Some(0));
    }

    #[test]
    fn merges_new_pages_without_erasing_the_last_advisory() {
        let mut state = AgentAttentionPanelState::default();
        assert_eq!(state.begin_poll(), Some(0));
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(
                2,
                2,
                Some(AdvisorySeverity::Warning),
                Some("Agent host is saturated."),
                Some("retry"),
            ),
        });
        assert_eq!(state.begin_poll(), Some(2));
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(2, 0, None, None, None),
        });

        let merged = state.attention.as_ref().unwrap();
        assert_eq!(merged.cursor, 2);
        assert_eq!(merged.unread, 2);
        assert_eq!(
            merged.latest_summary.as_deref(),
            Some("Agent host is saturated.")
        );
        assert_eq!(merged.suggested_action.as_deref(), Some("retry"));
    }

    #[test]
    fn degraded_status_keeps_last_known_attention_and_cursor() {
        let mut state = AgentAttentionPanelState::default();
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(
                1,
                1,
                Some(AdvisorySeverity::Info),
                Some("Agent host is ready."),
                None,
            ),
        });
        assert_eq!(state.begin_poll(), Some(1));
        state.complete_poll(AgentAttentionPollResult::Degraded(
            AgentHostDegradedReason::TransportIo,
        ));

        assert!(matches!(state.status, AgentHostStatus::Degraded { .. }));
        assert_eq!(state.attention.as_ref().unwrap().cursor, 1);
        let AgentHostStatus::Degraded { reason } = &state.status else {
            unreachable!();
        };
        assert_eq!(*reason, AgentHostDegradedReason::TransportIo);
    }

    #[test]
    fn a_valid_new_advisory_replaces_its_action_and_sticks_truncation() {
        let mut current = Some(attention(
            3,
            1,
            Some(AdvisorySeverity::Warning),
            Some("Agent host is saturated."),
            Some("retry"),
        ));
        let mut incoming = attention(
            4,
            1,
            Some(AdvisorySeverity::Info),
            Some("Agent host is ready."),
            None,
        );
        incoming.history_truncated = true;
        assert_eq!(merge_attention(&mut current, incoming), Ok(()));

        let current = current.unwrap();
        assert_eq!(current.cursor, 4);
        assert_eq!(
            current.latest_summary.as_deref(),
            Some("Agent host is ready.")
        );
        assert_eq!(current.suggested_action, None);
        assert!(current.history_truncated);
    }

    #[test]
    fn cancelled_poll_does_not_touch_the_socket() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            poll_agent_attention(0, &cancel),
            AgentAttentionPollResult::Cancelled
        );
    }

    #[test]
    fn lifecycle_drop_cancels_outstanding_poll_tokens() {
        let observed = {
            let lifecycle = AgentAttentionPollLifecycle::new();
            let observed = lifecycle.token();
            assert!(!observed.is_cancelled());
            observed
        };
        assert!(observed.is_cancelled());
    }

    #[test]
    fn cancelled_result_releases_the_single_flight_slot() {
        let mut state = AgentAttentionPanelState::default();
        assert_eq!(state.begin_poll(), Some(0));
        state.complete_poll(AgentAttentionPollResult::Cancelled);
        assert!(!state.poll_in_flight());
        assert_eq!(state.begin_poll(), Some(0));
    }

    #[test]
    fn raw_error_paths_and_hostile_profiles_never_enter_state() {
        let secret = std::path::PathBuf::from("/home/user/private/secret.sock");
        let reason = safe_reason(&Error::AgentNotFound(secret));
        assert_eq!(reason, AgentHostDegradedReason::AgentUnavailable);
        assert!(!reason.message().contains("/home"));

        let hostile_profile = format!("secret\n\u{1b}[31m{}", "x".repeat(10_000));
        let profile = AgentHostProfile::from_untrusted(&hostile_profile);
        assert_eq!(profile, AgentHostProfile::Compatible);
        assert_eq!(profile.label(), "compatible");
        assert!(!profile.label().contains("secret"));

        let mut state = AgentAttentionPanelState::default();
        state.complete_poll(AgentAttentionPollResult::Degraded(reason));
        let debug = format!("{state:?}");
        assert!(!debug.contains("/home/user/private/secret.sock"));
        assert!(!debug.contains("secret\n"));
        assert!(!debug.contains("[31m"));
        assert!(debug.len() < 500);
    }

    #[test]
    fn stale_attention_result_degrades_without_changing_known_state() {
        let mut state = AgentAttentionPanelState::default();
        state.attention = Some(attention(
            9,
            2,
            Some(AdvisorySeverity::Warning),
            Some("Agent host is saturated."),
            Some("retry"),
        ));
        assert_eq!(state.begin_poll(), Some(9));
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(
                8,
                4,
                Some(AdvisorySeverity::Info),
                Some("Agent host is ready."),
                None,
            ),
        });

        assert_eq!(
            state.status,
            AgentHostStatus::Degraded {
                reason: AgentHostDegradedReason::InvalidAdvisoryCursor
            }
        );
        let current = state.attention.unwrap();
        assert_eq!(current.cursor, 9);
        assert_eq!(current.unread, 2);
        assert_eq!(
            current.latest_summary.as_deref(),
            Some("Agent host is saturated.")
        );
        assert_eq!(current.suggested_action.as_deref(), Some("retry"));
    }

    #[test]
    fn equal_cursor_with_new_events_is_rejected_fail_closed() {
        let mut state = AgentAttentionPanelState::default();
        state.attention = Some(attention(9, 2, None, None, None));
        assert_eq!(state.begin_poll(), Some(9));
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(9, 1, None, None, None),
        });

        assert_eq!(
            state.status,
            AgentHostStatus::Degraded {
                reason: AgentHostDegradedReason::InvalidAdvisoryCursor
            }
        );
        let current = state.attention.unwrap();
        assert_eq!(current.cursor, 9);
        assert_eq!(current.unread, 2);
    }

    #[test]
    fn empty_page_cannot_advance_the_cursor() {
        let mut state = AgentAttentionPanelState::default();
        state.attention = Some(attention(9, 2, None, None, None));
        assert_eq!(state.begin_poll(), Some(9));
        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(10, 0, None, None, None),
        });

        assert_eq!(
            state.status,
            AgentHostStatus::Degraded {
                reason: AgentHostDegradedReason::InvalidAdvisoryCursor
            }
        );
        assert_eq!(state.attention.unwrap().cursor, 9);
    }

    #[test]
    fn degraded_polls_back_off_to_a_cap_and_success_resets_cadence() {
        let mut state = AgentAttentionPanelState::default();
        assert_eq!(state.next_poll_delay(), Duration::from_secs(5));
        state.complete_poll(AgentAttentionPollResult::Degraded(
            AgentHostDegradedReason::AgentUnavailable,
        ));
        assert_eq!(state.next_poll_delay(), Duration::from_secs(10));
        for _ in 0..10 {
            state.complete_poll(AgentAttentionPollResult::Degraded(
                AgentHostDegradedReason::AgentUnavailable,
            ));
        }
        assert_eq!(state.next_poll_delay(), MAX_POLL_BACKOFF);

        state.complete_poll(AgentAttentionPollResult::Ready {
            profile: AgentHostProfile::Desktop,
            attention: attention(0, 0, None, None, None),
        });
        assert_eq!(state.next_poll_delay(), HEALTHY_POLL_INTERVAL);
    }
}
