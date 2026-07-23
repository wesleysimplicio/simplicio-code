//! Pure, fail-closed projection for the shared operator inbox.
//!
//! This module deliberately has no transport and no execution callback.  It
//! turns canonical Agent/Loop/Runtime events into UI state and produces a
//! dispatch request only after a matching human authorization and
//! confirmation. Runtime/Loop receipts remain the sole authority for effects.

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

const MAX_TEXT_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatorAction {
    Approve,
    Reject,
    Pause,
    Resume,
    Redirect,
    Cancel,
    RetryAfterReconcile,
    Compare,
    RequestReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemStatus {
    NeedsDecision,
    AwaitingAuthorization,
    AwaitingConfirmation,
    ReadyToDispatch,
    AwaitingReceipt,
    Applied,
    Rejected,
    FailedClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub session_id: String,
    pub target: String,
    pub summary: String,
    pub risk: Risk,
    pub blocked: bool,
    pub approval_required: bool,
    pub interactive: bool,
    pub sequence: u64,
    pub status: ItemStatus,
    pub intent_id: Option<String>,
    pub receipt_id: Option<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionIntent {
    pub intent_id: String,
    pub session_id: String,
    pub target: String,
    pub action: OperatorAction,
    /// Callers must affirm this property; the reducer never guesses it.
    pub explicitly_idempotent: bool,
    pub idempotency_key: String,
    pub policy_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    pub intent_id: String,
    pub decision_id: String,
    pub actor: String,
    pub target: String,
    pub policy_revision: u64,
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchRequest {
    pub intent: ActionIntent,
    pub decision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalEvent {
    Attention {
        event_id: String,
        session_id: String,
        target: String,
        summary: String,
        risk: Risk,
        blocked: bool,
        approval_required: bool,
        interactive: bool,
        sequence: u64,
        evidence_refs: Vec<String>,
    },
    Authorization {
        event_id: String,
        authorization: Authorization,
    },
    Receipt {
        event_id: String,
        intent_id: String,
        decision_id: String,
        receipt_id: String,
        target: String,
        policy_revision: u64,
        applied: bool,
    },
}

impl CanonicalEvent {
    fn id(&self) -> &str {
        match self {
            Self::Attention { event_id, .. }
            | Self::Authorization { event_id, .. }
            | Self::Receipt { event_id, .. } => event_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboxError {
    #[error("invalid or unsafe inbox field: {0}")]
    InvalidField(&'static str),
    #[error("stale or conflicting event rejected")]
    StaleEvent,
    #[error("unknown intent rejected")]
    UnknownIntent,
    #[error("authorization does not match intent")]
    ForgedAuthorization,
    #[error("confirmation is required for this risk")]
    ConfirmationRequired,
    #[error("receipt does not match authorized dispatch")]
    ForgedReceipt,
    #[error("batch contains an intent not explicitly idempotent")]
    UnsafeBatch,
}

#[derive(Debug, Clone)]
struct Decision {
    intent: ActionIntent,
    authorization: Option<Authorization>,
    confirmed: bool,
    dispatched: bool,
}

#[derive(Debug, Default)]
pub struct OperationalInbox {
    items: BTreeMap<String, InboxItem>,
    decisions: BTreeMap<String, Decision>,
    seen_events: BTreeMap<String, CanonicalEvent>,
}

impl OperationalInbox {
    pub fn ordered_items(&self) -> Vec<&InboxItem> {
        let mut items: Vec<_> = self.items.values().collect();
        items.sort_by_key(|item| {
            (
                Reverse(item.risk),
                Reverse(item.blocked),
                Reverse(item.approval_required),
                Reverse(item.interactive),
                item.sequence,
                item.session_id.as_str(),
            )
        });
        items
    }

    pub fn apply(&mut self, event: CanonicalEvent) -> Result<ApplyOutcome, InboxError> {
        validate_token("event_id", event.id())?;
        if let Some(previous) = self.seen_events.get(event.id()) {
            return if previous == &event {
                Ok(ApplyOutcome::Duplicate)
            } else {
                Err(InboxError::StaleEvent)
            };
        }
        match &event {
            CanonicalEvent::Attention {
                session_id,
                target,
                summary,
                risk,
                blocked,
                approval_required,
                interactive,
                sequence,
                evidence_refs,
                ..
            } => {
                validate_token("session_id", session_id)?;
                validate_token("target", target)?;
                let summary = sanitize_text(summary)?;
                for value in evidence_refs {
                    validate_token("evidence_ref", value)?;
                }
                if self
                    .items
                    .get(session_id)
                    .is_some_and(|old| *sequence <= old.sequence)
                {
                    return Err(InboxError::StaleEvent);
                }
                self.items.insert(
                    session_id.clone(),
                    InboxItem {
                        session_id: session_id.clone(),
                        target: target.clone(),
                        summary,
                        risk: *risk,
                        blocked: *blocked,
                        approval_required: *approval_required,
                        interactive: *interactive,
                        sequence: *sequence,
                        status: ItemStatus::NeedsDecision,
                        intent_id: None,
                        receipt_id: None,
                        evidence_refs: evidence_refs.clone(),
                    },
                );
            }
            CanonicalEvent::Authorization { authorization, .. } => {
                validate_authorization(authorization)?;
                let decision = self
                    .decisions
                    .get_mut(&authorization.intent_id)
                    .ok_or(InboxError::UnknownIntent)?;
                if authorization.target != decision.intent.target
                    || authorization.policy_revision != decision.intent.policy_revision
                {
                    return Err(InboxError::ForgedAuthorization);
                }
                let item = self
                    .items
                    .get_mut(&decision.intent.session_id)
                    .ok_or(InboxError::UnknownIntent)?;
                if item.intent_id.as_deref() != Some(authorization.intent_id.as_str()) {
                    return Err(InboxError::StaleEvent);
                }
                if decision.authorization.is_some() {
                    return Err(InboxError::StaleEvent);
                }
                decision.authorization = Some(authorization.clone());
                item.status = if authorization.approved {
                    if needs_confirmation(item.risk, decision.intent.action) {
                        ItemStatus::AwaitingConfirmation
                    } else {
                        ItemStatus::ReadyToDispatch
                    }
                } else {
                    ItemStatus::Rejected
                };
            }
            CanonicalEvent::Receipt {
                intent_id,
                decision_id,
                receipt_id,
                target,
                policy_revision,
                applied,
                ..
            } => {
                validate_token("receipt_id", receipt_id)?;
                let decision = self
                    .decisions
                    .get_mut(intent_id)
                    .ok_or(InboxError::UnknownIntent)?;
                let authorization = decision
                    .authorization
                    .as_ref()
                    .ok_or(InboxError::ForgedReceipt)?;
                if !decision.dispatched
                    || authorization.decision_id != *decision_id
                    || decision.intent.target != *target
                    || decision.intent.policy_revision != *policy_revision
                {
                    return Err(InboxError::ForgedReceipt);
                }
                let item = self
                    .items
                    .get_mut(&decision.intent.session_id)
                    .ok_or(InboxError::UnknownIntent)?;
                if item.intent_id.as_deref() != Some(intent_id.as_str()) || item.receipt_id.is_some() {
                    return Err(InboxError::StaleEvent);
                }
                item.receipt_id = Some(receipt_id.clone());
                item.status = if *applied {
                    ItemStatus::Applied
                } else {
                    ItemStatus::FailedClosed
                };
            }
        }
        self.seen_events.insert(event.id().to_owned(), event);
        Ok(ApplyOutcome::Applied)
    }

    pub fn record_intent(&mut self, intent: ActionIntent) -> Result<(), InboxError> {
        validate_intent(&intent)?;
        if self.decisions.contains_key(&intent.intent_id) {
            return Err(InboxError::StaleEvent);
        }
        let item = self
            .items
            .get_mut(&intent.session_id)
            .ok_or(InboxError::UnknownIntent)?;
        if item.target != intent.target || item.status != ItemStatus::NeedsDecision {
            return Err(InboxError::StaleEvent);
        }
        item.intent_id = Some(intent.intent_id.clone());
        item.status = ItemStatus::AwaitingAuthorization;
        self.decisions.insert(
            intent.intent_id.clone(),
            Decision {
                intent,
                authorization: None,
                confirmed: false,
                dispatched: false,
            },
        );
        Ok(())
    }

    pub fn confirm(&mut self, intent_id: &str) -> Result<(), InboxError> {
        let decision = self
            .decisions
            .get_mut(intent_id)
            .ok_or(InboxError::UnknownIntent)?;
        if !decision.authorization.as_ref().is_some_and(|a| a.approved) {
            return Err(InboxError::ForgedAuthorization);
        }
        decision.confirmed = true;
        self.items
            .get_mut(&decision.intent.session_id)
            .ok_or(InboxError::UnknownIntent)?
            .status = ItemStatus::ReadyToDispatch;
        Ok(())
    }

    /// Returns a request for the existing Loop/Runtime gate; it performs no effect.
    pub fn take_dispatch(&mut self, intent_id: &str) -> Result<DispatchRequest, InboxError> {
        let decision = self
            .decisions
            .get_mut(intent_id)
            .ok_or(InboxError::UnknownIntent)?;
        let authorization = decision
            .authorization
            .as_ref()
            .filter(|a| a.approved)
            .ok_or(InboxError::ForgedAuthorization)?;
        let item = self
            .items
            .get_mut(&decision.intent.session_id)
            .ok_or(InboxError::UnknownIntent)?;
        if needs_confirmation(item.risk, decision.intent.action) && !decision.confirmed {
            return Err(InboxError::ConfirmationRequired);
        }
        if decision.dispatched || item.status != ItemStatus::ReadyToDispatch {
            return Err(InboxError::StaleEvent);
        }
        decision.dispatched = true;
        item.status = ItemStatus::AwaitingReceipt;
        Ok(DispatchRequest {
            intent: decision.intent.clone(),
            decision_id: authorization.decision_id.clone(),
        })
    }

    pub fn validate_batch(intents: &[ActionIntent]) -> Result<(), InboxError> {
        if intents.iter().any(|intent| !intent.explicitly_idempotent) {
            return Err(InboxError::UnsafeBatch);
        }
        let mut keys = BTreeSet::new();
        if intents
            .iter()
            .any(|intent| !keys.insert(intent.idempotency_key.as_str()))
        {
            return Err(InboxError::UnsafeBatch);
        }
        intents.iter().try_for_each(validate_intent)
    }
}

fn needs_confirmation(risk: Risk, action: OperatorAction) -> bool {
    risk >= Risk::High || matches!(action, OperatorAction::Cancel | OperatorAction::Redirect)
}

fn validate_intent(value: &ActionIntent) -> Result<(), InboxError> {
    for (name, value) in [
        ("intent_id", &value.intent_id),
        ("session_id", &value.session_id),
        ("target", &value.target),
        ("idempotency_key", &value.idempotency_key),
    ] {
        validate_token(name, value)?;
    }
    Ok(())
}

fn validate_authorization(value: &Authorization) -> Result<(), InboxError> {
    for (name, value) in [
        ("intent_id", &value.intent_id),
        ("decision_id", &value.decision_id),
        ("actor", &value.actor),
        ("target", &value.target),
    ] {
        validate_token(name, value)?;
    }
    Ok(())
}

fn validate_token(name: &'static str, value: &str) -> Result<(), InboxError> {
    if value.is_empty()
        || value.len() > MAX_TEXT_BYTES
        || value.bytes().any(|b| b.is_ascii_control())
    {
        Err(InboxError::InvalidField(name))
    } else {
        Ok(())
    }
}

/// Removes terminal escapes and controls rather than allowing them onto any surface.
fn sanitize_text(value: &str) -> Result<String, InboxError> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(InboxError::InvalidField("summary"));
    }
    let sanitized: String = value
        .chars()
        .filter(|ch| !ch.is_control() && *ch != '\u{1b}')
        .collect();
    Ok(redact_sensitive(&sanitized))
}

fn redact_sensitive(value: &str) -> String {
    let mut redacted = value.to_owned();
    for marker in ["Bearer ", "api_key=", "apikey=", "password=", "secret=", "token="] {
        let lower = redacted.to_ascii_lowercase();
        let Some(start) = lower.find(&marker.to_ascii_lowercase()) else {
            continue;
        };
        let value_start = start + marker.len();
        let value_end = redacted[value_start..]
            .find(char::is_whitespace)
            .map_or(redacted.len(), |offset| value_start + offset);
        redacted.replace_range(value_start..value_end, "[REDACTED]");
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attention(id: &str, session: &str, risk: Risk, sequence: u64) -> CanonicalEvent {
        CanonicalEvent::Attention {
            event_id: id.into(),
            session_id: session.into(),
            target: format!("run/{session}"),
            summary: "needs decision".into(),
            risk,
            blocked: risk >= Risk::High,
            approval_required: true,
            interactive: true,
            sequence,
            evidence_refs: vec!["receipt/ref".into()],
        }
    }
    fn intent(session: &str) -> ActionIntent {
        ActionIntent {
            intent_id: format!("intent-{session}"),
            session_id: session.into(),
            target: format!("run/{session}"),
            action: OperatorAction::Cancel,
            explicitly_idempotent: true,
            idempotency_key: format!("key-{session}"),
            policy_revision: 7,
        }
    }

    #[test]
    fn twenty_sessions_are_stably_risk_ordered() {
        let mut inbox = OperationalInbox::default();
        for n in 0..20 {
            inbox
                .apply(attention(
                    &format!("event-{n}"),
                    &format!("s{n:02}"),
                    if n == 19 { Risk::Critical } else { Risk::Low },
                    1,
                ))
                .unwrap();
        }
        assert_eq!(inbox.ordered_items().len(), 20);
        assert_eq!(inbox.ordered_items()[0].session_id, "s19");
    }

    #[test]
    fn effect_waits_for_authorization_confirmation_and_matching_receipt() {
        let mut inbox = OperationalInbox::default();
        inbox
            .apply(attention("event-1", "s1", Risk::Critical, 1))
            .unwrap();
        inbox.record_intent(intent("s1")).unwrap();
        let authorization = Authorization {
            intent_id: "intent-s1".into(),
            decision_id: "decision-1".into(),
            actor: "operator@example".into(),
            target: "run/s1".into(),
            policy_revision: 7,
            approved: true,
        };
        inbox
            .apply(CanonicalEvent::Authorization {
                event_id: "event-2".into(),
                authorization,
            })
            .unwrap();
        assert_eq!(
            inbox.take_dispatch("intent-s1"),
            Err(InboxError::ConfirmationRequired)
        );
        inbox.confirm("intent-s1").unwrap();
        assert_eq!(
            inbox.take_dispatch("intent-s1").unwrap().decision_id,
            "decision-1"
        );
        assert_eq!(inbox.ordered_items()[0].status, ItemStatus::AwaitingReceipt);
        inbox
            .apply(CanonicalEvent::Receipt {
                event_id: "event-3".into(),
                intent_id: "intent-s1".into(),
                decision_id: "decision-1".into(),
                receipt_id: "runtime-receipt".into(),
                target: "run/s1".into(),
                policy_revision: 7,
                applied: true,
            })
            .unwrap();
        assert_eq!(inbox.ordered_items()[0].status, ItemStatus::Applied);
    }

    #[test]
    fn stale_forged_duplicate_and_unsafe_batch_fail_closed() {
        let mut inbox = OperationalInbox::default();
        let first = attention("event-1", "s1", Risk::Low, 3);
        inbox.apply(first.clone()).unwrap();
        assert_eq!(inbox.apply(first), Ok(ApplyOutcome::Duplicate));
        assert_eq!(
            inbox.apply(attention("event-1", "s2", Risk::Low, 1)),
            Err(InboxError::StaleEvent)
        );
        assert_eq!(
            inbox.apply(attention("event-2", "s1", Risk::Critical, 2)),
            Err(InboxError::StaleEvent)
        );
        let mut unsafe_intent = intent("s1");
        unsafe_intent.explicitly_idempotent = false;
        assert_eq!(
            OperationalInbox::validate_batch(&[unsafe_intent]),
            Err(InboxError::UnsafeBatch)
        );
    }

    #[test]
    fn terminal_escape_is_removed_from_operator_text() {
        let mut inbox = OperationalInbox::default();
        let mut event = attention("event-1", "s1", Risk::Low, 1);
        if let CanonicalEvent::Attention { summary, .. } = &mut event {
            *summary = "ok\u{1b}[2J\nnext".into();
        }
        inbox.apply(event).unwrap();
        assert_eq!(inbox.ordered_items()[0].summary, "ok[2Jnext");
    }

    #[test]
    fn duplicate_intents_and_receipts_cannot_replace_decisions() {
        let mut inbox = OperationalInbox::default();
        inbox.apply(attention("event-1", "s1", Risk::Low, 1)).unwrap();
        inbox.record_intent(intent("s1")).unwrap();
        assert_eq!(inbox.record_intent(intent("s1")), Err(InboxError::StaleEvent));
        inbox
            .apply(CanonicalEvent::Authorization {
                event_id: "event-2".into(),
                authorization: Authorization {
                    intent_id: "intent-s1".into(),
                    decision_id: "decision-1".into(),
                    actor: "operator".into(),
                    target: "run/s1".into(),
                    policy_revision: 7,
                    approved: true,
                },
            })
            .unwrap();
        inbox.confirm("intent-s1").unwrap();
        inbox.take_dispatch("intent-s1").unwrap();
        let receipt = |event_id: &str, receipt_id: &str, applied: bool| CanonicalEvent::Receipt {
            event_id: event_id.into(),
            intent_id: "intent-s1".into(),
            decision_id: "decision-1".into(),
            receipt_id: receipt_id.into(),
            target: "run/s1".into(),
            policy_revision: 7,
            applied,
        };
        inbox.apply(receipt("event-3", "receipt-1", true)).unwrap();
        assert_eq!(
            inbox.apply(receipt("event-4", "receipt-2", false)),
            Err(InboxError::StaleEvent)
        );
        assert_eq!(inbox.ordered_items()[0].status, ItemStatus::Applied);
        assert_eq!(inbox.ordered_items()[0].receipt_id.as_deref(), Some("receipt-1"));
    }

    #[test]
    fn newer_attention_invalidates_old_authorization_and_receipt() {
        let mut inbox = OperationalInbox::default();
        inbox.apply(attention("event-1", "s1", Risk::Low, 1)).unwrap();
        inbox.record_intent(intent("s1")).unwrap();
        inbox
            .apply(CanonicalEvent::Authorization {
                event_id: "event-2".into(),
                authorization: Authorization {
                    intent_id: "intent-s1".into(),
                    decision_id: "decision-1".into(),
                    actor: "operator".into(),
                    target: "run/s1".into(),
                    policy_revision: 7,
                    approved: true,
                },
            })
            .unwrap();
        inbox.confirm("intent-s1").unwrap();
        inbox.take_dispatch("intent-s1").unwrap();
        inbox.apply(attention("event-3", "s1", Risk::Low, 2)).unwrap();
        assert_eq!(
            inbox.apply(CanonicalEvent::Receipt {
                event_id: "event-4".into(),
                intent_id: "intent-s1".into(),
                decision_id: "decision-1".into(),
                receipt_id: "late".into(),
                target: "run/s1".into(),
                policy_revision: 7,
                applied: true,
            }),
            Err(InboxError::StaleEvent)
        );
        assert_eq!(inbox.ordered_items()[0].status, ItemStatus::NeedsDecision);
    }

    #[test]
    fn sensitive_summary_values_are_redacted() {
        let mut inbox = OperationalInbox::default();
        let mut event = attention("event-1", "s1", Risk::Low, 1);
        if let CanonicalEvent::Attention { summary, .. } = &mut event {
            *summary = "Bearer abc token=xyz api_key=key".into();
        }
        inbox.apply(event).unwrap();
        assert_eq!(
            inbox.ordered_items()[0].summary,
            "Bearer [REDACTED] token=[REDACTED] api_key=[REDACTED]"
        );
    }
}
