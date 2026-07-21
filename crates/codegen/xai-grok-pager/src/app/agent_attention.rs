//! Passive projection of the independently shipped Simplicio Agent host.
//!
//! The pager polls only the existing `host.status` and `host.advisories`
//! operations. It never turns an advisory into an effect: this state is a
//! read-only input to the side panel.

use simplicio_agent_client::{AgentAttentionState, AgentHostClient, Error, HostInstanceId};
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
        request: AgentAttentionPollRequest,
        host_instance_id: HostInstanceId,
        replayed_from_cursor: u64,
        profile: AgentHostProfile,
        attention: AgentAttentionState,
    },
    Degraded {
        request: AgentAttentionPollRequest,
        reason: AgentHostDegradedReason,
    },
    Cancelled {
        request: AgentAttentionPollRequest,
    },
}

impl AgentAttentionPollResult {
    fn request(&self) -> &AgentAttentionPollRequest {
        match self {
            Self::Ready { request, .. }
            | Self::Degraded { request, .. }
            | Self::Cancelled { request } => request,
        }
    }
}

/// Single-flight ticket binding a poll to the exact state snapshot it extends.
/// The opaque instance identity remains redacted by the client newtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttentionPollRequest {
    generation: u64,
    cursor: u64,
    expected_host_instance_id: Option<HostInstanceId>,
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
    InvalidHostInstanceId,
    HostInstanceMismatch,
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
            Self::InvalidHostInstanceId => "invalid_host_instance_id",
            Self::HostInstanceMismatch => "host_instance_mismatch",
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
            Self::InvalidHostInstanceId => "Agent host identity was invalid.",
            Self::HostInstanceMismatch => "Agent host identity changed during polling.",
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

/// Fixed marker shown after an atomically committed host-restart replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentAttentionResyncState {
    #[default]
    Stable,
    RestartResync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IncarnatedAttention {
    host_instance_id: HostInstanceId,
    attention: AgentAttentionState,
}

/// App-owned state for the non-focusable Agent panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttentionPanelState {
    pub status: AgentHostStatus,
    stream: Option<IncarnatedAttention>,
    pub resync: AgentAttentionResyncState,
    next_poll_generation: u64,
    in_flight_generation: Option<u64>,
    consecutive_failures: u8,
}

impl Default for AgentAttentionPanelState {
    fn default() -> Self {
        Self {
            status: AgentHostStatus::Connecting,
            stream: None,
            resync: AgentAttentionResyncState::Stable,
            next_poll_generation: 1,
            in_flight_generation: None,
            consecutive_failures: 0,
        }
    }
}

impl AgentAttentionPanelState {
    /// Claim the single poll slot and bind it to the current incarnation/cursor.
    /// `None` means another poll is already in flight.
    pub fn begin_poll(&mut self) -> Option<AgentAttentionPollRequest> {
        if self.in_flight_generation.is_some() {
            return None;
        }
        let generation = self.next_poll_generation;
        self.next_poll_generation = self.next_poll_generation.wrapping_add(1);
        self.in_flight_generation = Some(generation);
        Some(AgentAttentionPollRequest {
            generation,
            cursor: self
                .stream
                .as_ref()
                .map_or(0, |stream| stream.attention.cursor),
            expected_host_instance_id: self
                .stream
                .as_ref()
                .map(|stream| stream.host_instance_id.clone()),
        })
    }

    pub fn attention(&self) -> Option<&AgentAttentionState> {
        self.stream.as_ref().map(|stream| &stream.attention)
    }

    /// Healthy cadence with capped exponential backoff after failures.
    pub fn next_poll_delay(&self) -> Duration {
        let multiplier = 1u32 << self.consecutive_failures.min(4);
        HEALTHY_POLL_INTERVAL
            .saturating_mul(multiplier)
            .min(MAX_POLL_BACKOFF)
    }

    pub fn complete_poll(&mut self, result: AgentAttentionPollResult) {
        if self.in_flight_generation != Some(result.request().generation) {
            return;
        }
        self.in_flight_generation = None;
        match result {
            AgentAttentionPollResult::Ready {
                request,
                host_instance_id,
                replayed_from_cursor,
                profile,
                attention,
            } => self.complete_ready_poll(
                request,
                host_instance_id,
                replayed_from_cursor,
                profile,
                attention,
            ),
            AgentAttentionPollResult::Degraded { reason, .. } => {
                self.record_failure(reason);
            }
            AgentAttentionPollResult::Cancelled { .. } => {}
        }
    }

    fn complete_ready_poll(
        &mut self,
        request: AgentAttentionPollRequest,
        host_instance_id: HostInstanceId,
        replayed_from_cursor: u64,
        profile: AgentHostProfile,
        attention: AgentAttentionState,
    ) {
        let current_identity = self.stream.as_ref().map(|stream| &stream.host_instance_id);
        if request.expected_host_instance_id.as_ref() != current_identity {
            self.record_failure(AgentHostDegradedReason::HostInstanceMismatch);
            return;
        }
        let current_cursor = self
            .stream
            .as_ref()
            .map_or(0, |stream| stream.attention.cursor);
        if request.cursor != current_cursor {
            self.record_failure(AgentHostDegradedReason::InvalidAdvisoryCursor);
            return;
        }

        let replacing_incarnation = current_identity.is_some_and(|id| id != &host_instance_id);
        let expected_replay_cursor = if replacing_incarnation {
            0
        } else {
            request.cursor
        };
        if replayed_from_cursor != expected_replay_cursor {
            self.record_failure(AgentHostDegradedReason::InvalidAdvisoryCursor);
            return;
        }

        let mut candidate = if replacing_incarnation {
            None
        } else {
            self.stream.as_ref().map(|stream| stream.attention.clone())
        };
        if merge_attention(&mut candidate, attention).is_err() {
            self.record_failure(AgentHostDegradedReason::InvalidAdvisoryCursor);
            return;
        }
        let attention = candidate.expect("a valid poll always initializes attention");
        self.stream = Some(IncarnatedAttention {
            host_instance_id,
            attention,
        });
        self.resync = if replacing_incarnation {
            AgentAttentionResyncState::RestartResync
        } else {
            AgentAttentionResyncState::Stable
        };
        self.consecutive_failures = 0;
        self.status = AgentHostStatus::Ready { profile };
    }

    fn record_failure(&mut self, reason: AgentHostDegradedReason) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.status = AgentHostStatus::Degraded { reason };
    }

    #[cfg(test)]
    fn poll_in_flight(&self) -> bool {
        self.in_flight_generation.is_some()
    }
}

/// Run one host poll off the event-loop thread.
///
/// Each transport request is already bounded by
/// `simplicio_agent_client::DEFAULT_REQUEST_TIMEOUT_MS` and the client's
/// response-size cap. Cancellation is checked before status, between status
/// and advisories, and after advisories; an in-progress blocking socket read
/// therefore finishes no later than its transport timeout.
pub fn poll_agent_attention(
    request: AgentAttentionPollRequest,
    cancel: &CancellationToken,
) -> AgentAttentionPollResult {
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled { request };
    }
    let client = match AgentHostClient::connect_default() {
        Ok(client) => client,
        Err(error) => {
            return AgentAttentionPollResult::Degraded {
                request,
                reason: safe_reason(&error),
            };
        }
    };
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled { request };
    }
    let profile = AgentHostProfile::from_untrusted(&client.capabilities().profile);
    let host_instance_id = client.capabilities().host_instance_id().clone();
    let replayed_from_cursor = match request.expected_host_instance_id.as_ref() {
        Some(expected) if expected != &host_instance_id => 0,
        _ => request.cursor,
    };
    let page = match client.advisories(replayed_from_cursor) {
        Ok(page) => page,
        Err(error) => {
            return AgentAttentionPollResult::Degraded {
                request,
                reason: safe_reason(&error),
            };
        }
    };
    if cancel.is_cancelled() {
        return AgentAttentionPollResult::Cancelled { request };
    }
    AgentAttentionPollResult::Ready {
        request,
        host_instance_id,
        replayed_from_cursor,
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
        || (event_count > 0
            && (advanced_by == 0
                || event_count > advanced_by
                || (event_count < advanced_by && !incoming.history_truncated)))
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
        Error::InvalidResponse(_) | Error::InvalidTurnRequest(_) => {
            AgentHostDegradedReason::InvalidResponse
        }
        Error::OperationRejected => AgentHostDegradedReason::HostNotReady,
        Error::ProtocolMismatch(_) => AgentHostDegradedReason::ProtocolMismatch,
        Error::CapabilityMismatch { .. } => AgentHostDegradedReason::CapabilityMismatch,
        Error::InvalidHostInstanceId => AgentHostDegradedReason::InvalidHostInstanceId,
        Error::HostInstanceMismatch => AgentHostDegradedReason::HostInstanceMismatch,
        Error::InvalidAdvisoryCursor => AgentHostDegradedReason::InvalidAdvisoryCursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simplicio_agent_client::AdvisorySeverity;

    const HOST_A: &str = "host-instance-aaaaaaaa";
    const HOST_B: &str = "host-instance-bbbbbbbb";

    fn host_id(value: &str) -> HostInstanceId {
        HostInstanceId::from_untrusted(value).unwrap()
    }

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

    fn ready_result(
        request: AgentAttentionPollRequest,
        host: &str,
        replayed_from_cursor: u64,
        attention: AgentAttentionState,
    ) -> AgentAttentionPollResult {
        AgentAttentionPollResult::Ready {
            request,
            host_instance_id: host_id(host),
            replayed_from_cursor,
            profile: AgentHostProfile::Desktop,
            attention,
        }
    }

    fn complete_ready(
        state: &mut AgentAttentionPanelState,
        host: &str,
        attention: AgentAttentionState,
    ) {
        let request = state.begin_poll().unwrap();
        let current = host_id(host);
        let replayed_from_cursor = match request.expected_host_instance_id.as_ref() {
            Some(expected) if expected != &current => 0,
            _ => request.cursor,
        };
        state.complete_poll(ready_result(request, host, replayed_from_cursor, attention));
    }

    #[test]
    fn admits_only_one_incarnation_bound_poll_at_a_time() {
        let mut state = AgentAttentionPanelState::default();
        let request = state.begin_poll().unwrap();
        assert_eq!(request.cursor, 0);
        assert!(request.expected_host_instance_id.is_none());
        assert_eq!(state.begin_poll(), None);
        assert!(state.poll_in_flight());

        state.complete_poll(AgentAttentionPollResult::Degraded {
            request,
            reason: AgentHostDegradedReason::AgentUnavailable,
        });
        assert!(!state.poll_in_flight());
        assert_eq!(state.begin_poll().unwrap().cursor, 0);
    }

    #[test]
    fn first_poll_and_same_incarnation_merge_without_false_resync() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            attention(
                2,
                2,
                Some(AdvisorySeverity::Warning),
                Some("Agent host is saturated."),
                Some("retry"),
            ),
        );
        assert_eq!(state.resync, AgentAttentionResyncState::Stable);

        let request = state.begin_poll().unwrap();
        assert_eq!(request.cursor, 2);
        assert_eq!(
            request.expected_host_instance_id.as_ref(),
            Some(&host_id(HOST_A))
        );
        state.complete_poll(ready_result(
            request,
            HOST_A,
            2,
            attention(2, 0, None, None, None),
        ));

        let merged = state.attention().unwrap();
        assert_eq!(merged.cursor, 2);
        assert_eq!(merged.unread, 2);
        assert_eq!(
            merged.latest_summary.as_deref(),
            Some("Agent host is saturated.")
        );
        assert_eq!(merged.suggested_action.as_deref(), Some("retry"));
        assert_eq!(state.resync, AgentAttentionResyncState::Stable);
    }

    #[test]
    fn proven_restart_replays_zero_and_atomically_replaces_equal_numeric_cursor() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            attention(
                2,
                2,
                Some(AdvisorySeverity::Warning),
                Some("Agent host is saturated."),
                Some("retry"),
            ),
        );

        let request = state.begin_poll().unwrap();
        state.complete_poll(ready_result(
            request,
            HOST_B,
            0,
            attention(
                2,
                2,
                Some(AdvisorySeverity::Info),
                Some("Agent host is ready."),
                None,
            ),
        ));

        assert_eq!(state.resync, AgentAttentionResyncState::RestartResync);
        let replaced = state.attention().unwrap();
        assert_eq!(replaced.cursor, 2);
        assert_eq!(replaced.unread, 2);
        assert_eq!(replaced.highest_severity, Some(AdvisorySeverity::Info));
        assert_eq!(
            replaced.latest_summary.as_deref(),
            Some("Agent host is ready.")
        );
        assert_eq!(replaced.suggested_action, None);

        complete_ready(&mut state, HOST_B, attention(2, 0, None, None, None));
        assert_eq!(state.resync, AgentAttentionResyncState::Stable);
    }

    #[test]
    fn restart_requires_a_validated_cursor_zero_replay() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            attention(
                1,
                1,
                Some(AdvisorySeverity::Warning),
                Some("Agent host is saturated."),
                Some("retry"),
            ),
        );
        let request = state.begin_poll().unwrap();
        state.complete_poll(ready_result(
            request,
            HOST_B,
            1,
            attention(1, 1, Some(AdvisorySeverity::Info), None, None),
        ));

        assert_eq!(
            state.status,
            AgentHostStatus::Degraded {
                reason: AgentHostDegradedReason::InvalidAdvisoryCursor
            }
        );
        assert_eq!(
            state.attention().unwrap().latest_summary.as_deref(),
            Some("Agent host is saturated.")
        );
        assert_eq!(state.resync, AgentAttentionResyncState::Stable);
    }

    #[test]
    fn delayed_old_incarnation_result_cannot_regress_committed_restart() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            attention(
                1,
                1,
                Some(AdvisorySeverity::Warning),
                Some("Agent host is saturated."),
                None,
            ),
        );
        let old_request = state.begin_poll().unwrap();
        state.complete_poll(ready_result(
            old_request.clone(),
            HOST_B,
            0,
            attention(
                1,
                1,
                Some(AdvisorySeverity::Info),
                Some("Agent host is ready."),
                None,
            ),
        ));

        let live_request = state.begin_poll().unwrap();
        state.complete_poll(ready_result(
            old_request,
            HOST_A,
            1,
            attention(1, 0, None, None, None),
        ));

        assert!(matches!(&state.status, AgentHostStatus::Ready { .. }));
        assert!(state.poll_in_flight());
        assert_eq!(
            state.attention().unwrap().latest_summary.as_deref(),
            Some("Agent host is ready.")
        );
        assert_eq!(state.resync, AgentAttentionResyncState::RestartResync);

        state.complete_poll(ready_result(
            live_request,
            HOST_B,
            1,
            attention(1, 0, None, None, None),
        ));
        assert!(!state.poll_in_flight());
    }

    #[test]
    fn same_incarnation_stale_or_future_results_stay_fail_closed() {
        for invalid in [
            attention(0, 0, None, None, None),
            attention(3, 1, Some(AdvisorySeverity::Info), None, None),
        ] {
            let mut state = AgentAttentionPanelState::default();
            complete_ready(
                &mut state,
                HOST_A,
                attention(
                    1,
                    1,
                    Some(AdvisorySeverity::Warning),
                    Some("Agent host is saturated."),
                    None,
                ),
            );
            let request = state.begin_poll().unwrap();
            state.complete_poll(ready_result(request, HOST_A, 1, invalid));

            assert_eq!(
                state.status,
                AgentHostStatus::Degraded {
                    reason: AgentHostDegradedReason::InvalidAdvisoryCursor
                }
            );
            assert_eq!(state.attention().unwrap().cursor, 1);
            assert_eq!(state.attention().unwrap().unread, 1);
        }
    }

    #[test]
    fn degraded_status_keeps_last_known_incarnation_and_backoff_is_capped() {
        let mut state = AgentAttentionPanelState::default();
        complete_ready(
            &mut state,
            HOST_A,
            attention(1, 1, Some(AdvisorySeverity::Info), None, None),
        );
        let request = state.begin_poll().unwrap();
        state.complete_poll(AgentAttentionPollResult::Degraded {
            request,
            reason: AgentHostDegradedReason::TransportIo,
        });

        assert_eq!(state.attention().unwrap().cursor, 1);
        assert_eq!(state.next_poll_delay(), Duration::from_secs(10));
        for _ in 0..10 {
            let request = state.begin_poll().unwrap();
            state.complete_poll(AgentAttentionPollResult::Degraded {
                request,
                reason: AgentHostDegradedReason::AgentUnavailable,
            });
        }
        assert_eq!(state.next_poll_delay(), MAX_POLL_BACKOFF);

        let request = state.begin_poll().unwrap();
        state.complete_poll(ready_result(
            request,
            HOST_A,
            1,
            attention(1, 0, None, None, None),
        ));
        assert_eq!(state.next_poll_delay(), HEALTHY_POLL_INTERVAL);
    }

    #[test]
    fn truncation_is_the_only_valid_reason_for_a_sequence_gap() {
        let mut current = Some(attention(3, 1, None, None, None));
        let gap = attention(5, 1, Some(AdvisorySeverity::Info), None, None);
        assert_eq!(
            merge_attention(&mut current, gap.clone()),
            Err(AttentionMergeError::InvalidCursor)
        );

        let mut truncated_gap = gap;
        truncated_gap.history_truncated = true;
        assert_eq!(merge_attention(&mut current, truncated_gap), Ok(()));
        assert_eq!(current.unwrap().cursor, 5);
    }

    #[test]
    fn cancellation_releases_single_flight_and_can_precede_socket_access() {
        let mut state = AgentAttentionPanelState::default();
        let request = state.begin_poll().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = poll_agent_attention(request, &cancel);
        assert!(matches!(
            &result,
            AgentAttentionPollResult::Cancelled { .. }
        ));
        state.complete_poll(result);
        assert!(!state.poll_in_flight());
        assert_eq!(state.begin_poll().unwrap().cursor, 0);
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
    fn identity_and_raw_errors_never_enter_state_debug_or_ui_catalog() {
        let secret = "/home/user/private/secret.sock";
        let reason = safe_reason(&Error::AgentNotFound(secret.into()));
        assert_eq!(reason, AgentHostDegradedReason::AgentUnavailable);
        assert!(!reason.message().contains(secret));

        let mut state = AgentAttentionPanelState::default();
        complete_ready(&mut state, HOST_A, attention(0, 0, None, None, None));
        let debug = format!("{state:?}");
        assert!(!debug.contains(HOST_A));
        assert!(debug.contains("[redacted]"));

        assert_eq!(
            safe_reason(&Error::InvalidHostInstanceId),
            AgentHostDegradedReason::InvalidHostInstanceId
        );
        assert_eq!(
            safe_reason(&Error::HostInstanceMismatch),
            AgentHostDegradedReason::HostInstanceMismatch
        );
        assert_eq!(
            safe_reason(&Error::InvalidAdvisoryCursor),
            AgentHostDegradedReason::InvalidAdvisoryCursor
        );
    }
}
