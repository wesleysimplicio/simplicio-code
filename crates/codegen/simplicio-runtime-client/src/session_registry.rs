//! Provider-neutral canonical session index for external Agent/Loop/Runtime sessions.
//!
//! The registry stores only bounded identifiers, cursors, lifecycle state, and
//! redacted display metadata. It never owns a coordinator, model, provider,
//! Runtime, scheduler, filesystem, process, prompt, or conversation content.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SESSION_REGISTRY_SCHEMA_V1: &str = "simplicio.session-registry/v1";
pub const SESSION_REGISTRY_SCHEMA_V0: &str = "simplicio.session-registry/v0";
pub const MAX_SESSIONS: usize = 10_000;
pub const MAX_CLIENTS_PER_SESSION: usize = 16;
pub const MAX_REPLAY_EVENTS: u64 = 1_024;
const MAX_ID_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid session registry payload")]
    InvalidPayload,
    #[error("session is unknown")]
    UnknownSession,
    #[error("session already exists")]
    DuplicateSession,
    #[error("session registry capacity exceeded")]
    CapacityExceeded,
    #[error("invalid lifecycle transition")]
    InvalidTransition,
    #[error("event cursor is stale")]
    StaleCursor,
    #[error("event cursor is in the future")]
    FutureCursor,
    #[error("event replay exceeds the bounded window")]
    ReplayTooLarge,
    #[error("response belongs to a different host incarnation")]
    HostInstanceMismatch,
    #[error("response is older than the applied response")]
    DelayedResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Coordinator,
    Work,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Detached,
    Paused,
    Cancelling,
    Cancelled,
    Closed,
    WaitingApproval,
    Blocked,
    Failed,
    Done,
    Degraded,
}

impl SessionStatus {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled | Self::Closed | Self::Failed | Self::Done
        )
    }

    fn notification_kind(self) -> Option<NotificationKind> {
        match self {
            Self::WaitingApproval => Some(NotificationKind::WaitingApproval),
            Self::Blocked => Some(NotificationKind::Blocked),
            Self::Failed => Some(NotificationKind::Failed),
            Self::Done => Some(NotificationKind::Done),
            Self::Degraded => Some(NotificationKind::Degraded),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    WaitingApproval,
    Blocked,
    Failed,
    Done,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    HostRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradedReason {
    InvalidPayload,
    UnknownSession,
    StaleCursor,
    FutureCursor,
    ReplayTooLarge,
    HostInstanceMismatch,
    DelayedResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMetadata {
    pub space_id: String,
    pub project_id: String,
    pub workspace_id: String,
    pub agent_label: String,
    pub goal_label: String,
    pub branch_label: Option<String>,
    pub worktree_label: Option<String>,
}

impl SessionMetadata {
    pub fn validate(&self) -> Result<(), RegistryError> {
        validate_id(&self.space_id)?;
        validate_id(&self.project_id)?;
        validate_id(&self.workspace_id)?;
        validate_label(&self.agent_label)?;
        validate_label(&self.goal_label)?;
        if let Some(value) = &self.branch_label {
            validate_label(value)?;
        }
        if let Some(value) = &self.worktree_label {
            validate_label(value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub session_id: String,
    pub kind: SessionKind,
    pub metadata: SessionMetadata,
    pub status: SessionStatus,
    pub blocked: bool,
    pub last_activity_ms: u64,
    pub external_handle_id: String,
    pub host_instance_id: String,
    pub cursor: u64,
    pub response_sequence: u64,
    pub attached_clients: BTreeSet<String>,
    pub emitted_notifications: BTreeSet<NotificationKind>,
    #[serde(default)]
    pub resync_reason: Option<ResyncReason>,
    #[serde(default)]
    pub degraded_reason: Option<DegradedReason>,
}

impl SessionRecord {
    fn validate(&self) -> Result<(), RegistryError> {
        validate_id(&self.session_id)?;
        self.metadata.validate()?;
        validate_id(&self.external_handle_id)?;
        validate_id(&self.host_instance_id)?;
        if self.attached_clients.len() > MAX_CLIENTS_PER_SESSION {
            return Err(RegistryError::CapacityExceeded);
        }
        for client in &self.attached_clients {
            validate_id(client)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrySnapshot {
    pub schema: String,
    pub sessions: Vec<SessionRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionQuery<'a> {
    pub space_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub workspace_id: Option<&'a str>,
    pub status: Option<SessionStatus>,
    pub goal: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub session_id: String,
    pub kind: NotificationKind,
    /// Always false: registry notifications are passive projections.
    pub steals_focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectOutcome {
    Replay {
        after_cursor: u64,
        limit: u64,
    },
    Resync {
        from_cursor: u64,
        reason: ResyncReason,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SessionRegistry {
    sessions: BTreeMap<String, SessionRecord>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restore(snapshot: RegistrySnapshot) -> Result<Self, RegistryError> {
        if snapshot.schema != SESSION_REGISTRY_SCHEMA_V1
            && snapshot.schema != SESSION_REGISTRY_SCHEMA_V0
        {
            return Err(RegistryError::InvalidPayload);
        }
        if snapshot.sessions.len() > MAX_SESSIONS {
            return Err(RegistryError::CapacityExceeded);
        }
        let mut sessions = BTreeMap::new();
        for record in snapshot.sessions {
            record.validate()?;
            // v0 readers did not define resync/degraded metadata. The serde
            // shape is upgraded before restore by the external persistence adapter.
            if sessions.insert(record.session_id.clone(), record).is_some() {
                return Err(RegistryError::DuplicateSession);
            }
        }
        Ok(Self { sessions })
    }

    pub fn snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            schema: SESSION_REGISTRY_SCHEMA_V1.to_owned(),
            sessions: self.sessions.values().cloned().collect(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        session_id: String,
        kind: SessionKind,
        metadata: SessionMetadata,
        external_handle_id: String,
        host_instance_id: String,
        now_ms: u64,
    ) -> Result<&SessionRecord, RegistryError> {
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(RegistryError::CapacityExceeded);
        }
        validate_id(&session_id)?;
        metadata.validate()?;
        validate_id(&external_handle_id)?;
        validate_id(&host_instance_id)?;
        if self.sessions.contains_key(&session_id) {
            return Err(RegistryError::DuplicateSession);
        }
        let record = SessionRecord {
            session_id: session_id.clone(),
            kind,
            metadata,
            status: SessionStatus::Detached,
            blocked: false,
            last_activity_ms: now_ms,
            external_handle_id,
            host_instance_id,
            cursor: 0,
            response_sequence: 0,
            attached_clients: BTreeSet::new(),
            emitted_notifications: BTreeSet::new(),
            resync_reason: None,
            degraded_reason: None,
        };
        self.sessions.insert(session_id.clone(), record);
        Ok(&self.sessions[&session_id])
    }

    pub fn get(&self, session_id: &str) -> Result<&SessionRecord, RegistryError> {
        self.sessions
            .get(session_id)
            .ok_or(RegistryError::UnknownSession)
    }

    pub fn list<'a>(&'a self, query: &SessionQuery<'_>) -> Vec<&'a SessionRecord> {
        let goal = query.goal.map(str::to_ascii_lowercase);
        self.sessions
            .values()
            .filter(|record| {
                query.space_id.is_none_or(|v| record.metadata.space_id == v)
                    && query
                        .project_id
                        .is_none_or(|v| record.metadata.project_id == v)
                    && query
                        .workspace_id
                        .is_none_or(|v| record.metadata.workspace_id == v)
                    && query.status.is_none_or(|v| record.status == v)
                    && goal
                        .as_ref()
                        .is_none_or(|v| record.metadata.goal_label.to_ascii_lowercase().contains(v))
            })
            .collect()
    }

    /// Attaches another surface to the already-existing external handle.
    /// This method only returns that opaque handle; it cannot spawn anything.
    pub fn attach(
        &mut self,
        session_id: &str,
        client_id: &str,
        now_ms: u64,
    ) -> Result<String, RegistryError> {
        validate_id(client_id)?;
        let record = self.record_mut(session_id)?;
        if record.status.terminal() {
            return Err(RegistryError::InvalidTransition);
        }
        if !record.attached_clients.contains(client_id)
            && record.attached_clients.len() >= MAX_CLIENTS_PER_SESSION
        {
            return Err(RegistryError::CapacityExceeded);
        }
        record.attached_clients.insert(client_id.to_owned());
        record.status = SessionStatus::Active;
        record.last_activity_ms = record.last_activity_ms.max(now_ms);
        Ok(record.external_handle_id.clone())
    }

    pub fn detach(
        &mut self,
        session_id: &str,
        client_id: &str,
        now_ms: u64,
    ) -> Result<(), RegistryError> {
        let record = self.record_mut(session_id)?;
        record.attached_clients.remove(client_id);
        if record.attached_clients.is_empty() && record.status == SessionStatus::Active {
            record.status = SessionStatus::Detached;
        }
        record.last_activity_ms = record.last_activity_ms.max(now_ms);
        Ok(())
    }

    pub fn pause(&mut self, session_id: &str, now_ms: u64) -> Result<(), RegistryError> {
        self.transition(
            session_id,
            now_ms,
            &[SessionStatus::Active, SessionStatus::Detached],
            SessionStatus::Paused,
        )
    }

    pub fn resume(&mut self, session_id: &str, now_ms: u64) -> Result<(), RegistryError> {
        self.transition(
            session_id,
            now_ms,
            &[SessionStatus::Paused],
            SessionStatus::Detached,
        )
    }

    pub fn cancel(&mut self, session_id: &str, now_ms: u64) -> Result<(), RegistryError> {
        let status = self.get(session_id)?.status;
        if status == SessionStatus::Cancelled || status == SessionStatus::Cancelling {
            return Ok(());
        }
        self.transition(
            session_id,
            now_ms,
            &[
                SessionStatus::Active,
                SessionStatus::Detached,
                SessionStatus::Paused,
                SessionStatus::WaitingApproval,
                SessionStatus::Blocked,
                SessionStatus::Degraded,
            ],
            SessionStatus::Cancelling,
        )
    }

    pub fn close(&mut self, session_id: &str, now_ms: u64) -> Result<(), RegistryError> {
        let status = self.get(session_id)?.status;
        if status == SessionStatus::Closed {
            return Ok(());
        }
        self.transition(
            session_id,
            now_ms,
            &[
                SessionStatus::Cancelled,
                SessionStatus::Failed,
                SessionStatus::Done,
            ],
            SessionStatus::Closed,
        )
    }

    pub fn apply_status(
        &mut self,
        session_id: &str,
        status: SessionStatus,
        blocked: bool,
        now_ms: u64,
    ) -> Result<Option<Notification>, RegistryError> {
        let record = self.record_mut(session_id)?;
        if record.status == SessionStatus::Closed {
            return Err(RegistryError::InvalidTransition);
        }
        record.status = status;
        record.blocked = blocked || status == SessionStatus::Blocked;
        record.last_activity_ms = record.last_activity_ms.max(now_ms);
        record.degraded_reason = None;
        let Some(kind) = status.notification_kind() else {
            return Ok(None);
        };
        if !record.emitted_notifications.insert(kind) {
            return Ok(None);
        }
        Ok(Some(Notification {
            session_id: session_id.to_owned(),
            kind,
            steals_focus: false,
        }))
    }

    pub fn reconnect(
        &mut self,
        session_id: &str,
        discovered_host_instance_id: &str,
        requested_after_cursor: u64,
        replay_limit: u64,
    ) -> Result<ReconnectOutcome, RegistryError> {
        validate_id(discovered_host_instance_id)?;
        if replay_limit == 0 || replay_limit > MAX_REPLAY_EVENTS {
            return self.degrade(
                session_id,
                DegradedReason::ReplayTooLarge,
                RegistryError::ReplayTooLarge,
            );
        }
        let record = self.record_mut(session_id)?;
        if record.host_instance_id != discovered_host_instance_id {
            record.host_instance_id = discovered_host_instance_id.to_owned();
            record.cursor = 0;
            record.response_sequence = 0;
            record.resync_reason = Some(ResyncReason::HostRestart);
            record.degraded_reason = None;
            return Ok(ReconnectOutcome::Resync {
                from_cursor: 0,
                reason: ResyncReason::HostRestart,
            });
        }
        if requested_after_cursor < record.cursor {
            return self.degrade(
                session_id,
                DegradedReason::StaleCursor,
                RegistryError::StaleCursor,
            );
        }
        if requested_after_cursor > record.cursor {
            return self.degrade(
                session_id,
                DegradedReason::FutureCursor,
                RegistryError::FutureCursor,
            );
        }
        Ok(ReconnectOutcome::Replay {
            after_cursor: record.cursor,
            limit: replay_limit,
        })
    }

    pub fn apply_replay(
        &mut self,
        session_id: &str,
        host_instance_id: &str,
        response_sequence: u64,
        from_cursor: u64,
        to_cursor: u64,
    ) -> Result<(), RegistryError> {
        let record = self.record_mut(session_id)?;
        if record.host_instance_id != host_instance_id {
            return self.degrade(
                session_id,
                DegradedReason::HostInstanceMismatch,
                RegistryError::HostInstanceMismatch,
            );
        }
        if response_sequence < record.response_sequence {
            return self.degrade(
                session_id,
                DegradedReason::DelayedResponse,
                RegistryError::DelayedResponse,
            );
        }
        if from_cursor != record.cursor {
            let (reason, error) = if from_cursor < record.cursor {
                (DegradedReason::StaleCursor, RegistryError::StaleCursor)
            } else {
                (DegradedReason::FutureCursor, RegistryError::FutureCursor)
            };
            return self.degrade(session_id, reason, error);
        }
        let count = to_cursor
            .checked_sub(from_cursor)
            .ok_or(RegistryError::StaleCursor)?;
        if count > MAX_REPLAY_EVENTS {
            return self.degrade(
                session_id,
                DegradedReason::ReplayTooLarge,
                RegistryError::ReplayTooLarge,
            );
        }
        record.cursor = to_cursor;
        record.response_sequence = response_sequence;
        record.resync_reason = None;
        record.degraded_reason = None;
        Ok(())
    }

    fn transition(
        &mut self,
        session_id: &str,
        now_ms: u64,
        allowed: &[SessionStatus],
        target: SessionStatus,
    ) -> Result<(), RegistryError> {
        let record = self.record_mut(session_id)?;
        if record.status == target {
            return Ok(());
        }
        if !allowed.contains(&record.status) {
            return Err(RegistryError::InvalidTransition);
        }
        record.status = target;
        record.last_activity_ms = record.last_activity_ms.max(now_ms);
        if target == SessionStatus::Closed {
            record.attached_clients.clear();
        }
        Ok(())
    }

    fn record_mut(&mut self, session_id: &str) -> Result<&mut SessionRecord, RegistryError> {
        self.sessions
            .get_mut(session_id)
            .ok_or(RegistryError::UnknownSession)
    }

    fn degrade<T>(
        &mut self,
        session_id: &str,
        reason: DegradedReason,
        error: RegistryError,
    ) -> Result<T, RegistryError> {
        if let Ok(record) = self.record_mut(session_id) {
            record.status = SessionStatus::Degraded;
            record.degraded_reason = Some(reason);
        }
        Err(error)
    }
}

fn validate_id(value: &str) -> Result<(), RegistryError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RegistryError::InvalidPayload);
    }
    Ok(())
}

fn validate_label(value: &str) -> Result<(), RegistryError> {
    let lower = value.to_ascii_lowercase();
    let looks_secret = [
        "bearer ", "api_key", "apikey", "password", "secret=", "token=",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let looks_absolute_path = value.starts_with('/')
        || value.starts_with("\\\\")
        || value.as_bytes().get(1) == Some(&b':');
    if value.is_empty()
        || value.len() > MAX_LABEL_BYTES
        || value.chars().any(char::is_control)
        || looks_secret
        || looks_absolute_path
    {
        return Err(RegistryError::InvalidPayload);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};

    fn metadata(project: &str, goal: &str) -> SessionMetadata {
        SessionMetadata {
            space_id: "space".into(),
            project_id: project.into(),
            workspace_id: format!("ws-{project}"),
            agent_label: "external-agent".into(),
            goal_label: goal.into(),
            branch_label: Some("feature-safe".into()),
            worktree_label: Some("worktree-safe".into()),
        }
    }

    fn registry() -> SessionRegistry {
        let mut registry = SessionRegistry::new();
        registry
            .create(
                "s1".into(),
                SessionKind::Coordinator,
                metadata("p1", "ship registry"),
                "shared-handle".into(),
                "host-a".into(),
                1,
            )
            .unwrap();
        registry
    }

    #[test]
    fn lists_two_projects_by_project_status_and_goal() {
        let mut registry = registry();
        registry
            .create(
                "s2".into(),
                SessionKind::Work,
                metadata("p2", "fix parser"),
                "handle-2".into(),
                "host-b".into(),
                2,
            )
            .unwrap();
        assert_eq!(
            registry.list(&SessionQuery {
                project_id: Some("p2"),
                ..Default::default()
            })[0]
                .session_id,
            "s2"
        );
        assert_eq!(
            registry
                .list(&SessionQuery {
                    status: Some(SessionStatus::Detached),
                    ..Default::default()
                })
                .len(),
            2
        );
        assert_eq!(
            registry.list(&SessionQuery {
                goal: Some("REGISTRY"),
                ..Default::default()
            })[0]
                .session_id,
            "s1"
        );
    }

    #[test]
    fn surfaces_share_one_external_handle_and_attach_is_idempotent() {
        let mut registry = registry();
        for client in ["tui", "headless", "acp", "workspace"] {
            assert_eq!(registry.attach("s1", client, 2).unwrap(), "shared-handle");
        }
        assert_eq!(registry.attach("s1", "tui", 3).unwrap(), "shared-handle");
        assert_eq!(registry.get("s1").unwrap().attached_clients.len(), 4);
    }

    #[test]
    fn lifecycle_rejects_invalid_transitions_and_dedupes_cancel_close() {
        let mut registry = registry();
        assert_eq!(
            registry.close("s1", 2),
            Err(RegistryError::InvalidTransition)
        );
        registry.attach("s1", "tui", 2).unwrap();
        registry.pause("s1", 3).unwrap();
        registry.resume("s1", 4).unwrap();
        registry.cancel("s1", 5).unwrap();
        registry.cancel("s1", 6).unwrap();
        registry
            .apply_status("s1", SessionStatus::Cancelled, false, 7)
            .unwrap();
        registry.close("s1", 8).unwrap();
        registry.close("s1", 9).unwrap();
        assert_eq!(
            registry.attach("s1", "acp", 10),
            Err(RegistryError::InvalidTransition)
        );
    }

    #[test]
    fn restart_resyncs_and_delayed_old_host_response_cannot_regress_cursor() {
        let mut registry = registry();
        registry.apply_replay("s1", "host-a", 1, 0, 5).unwrap();
        assert_eq!(
            registry.reconnect("s1", "host-b", 5, 10).unwrap(),
            ReconnectOutcome::Resync {
                from_cursor: 0,
                reason: ResyncReason::HostRestart
            }
        );
        assert_eq!(
            registry.apply_replay("s1", "host-a", 2, 5, 6),
            Err(RegistryError::HostInstanceMismatch)
        );
        assert_eq!(registry.get("s1").unwrap().cursor, 0);
    }

    #[test]
    fn stale_future_and_oversized_replay_fail_closed() {
        let mut registry = registry();
        registry.apply_replay("s1", "host-a", 1, 0, 5).unwrap();
        assert_eq!(
            registry.reconnect("s1", "host-a", 4, 10),
            Err(RegistryError::StaleCursor)
        );
        assert_eq!(
            registry.reconnect("s1", "host-a", 6, 10),
            Err(RegistryError::FutureCursor)
        );
        assert_eq!(
            registry.reconnect("s1", "host-a", 5, MAX_REPLAY_EVENTS + 1),
            Err(RegistryError::ReplayTooLarge)
        );
        assert_eq!(registry.get("s1").unwrap().cursor, 5);
    }

    #[test]
    fn notifications_are_passive_and_idempotent() {
        let mut registry = registry();
        let first = registry
            .apply_status("s1", SessionStatus::WaitingApproval, false, 2)
            .unwrap()
            .unwrap();
        assert!(!first.steals_focus);
        assert!(
            registry
                .apply_status("s1", SessionStatus::WaitingApproval, false, 3)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn snapshot_roundtrip_and_n_minus_one_defaults_are_compatible() {
        let registry = registry();
        let json = serde_json::to_string(&registry.snapshot()).unwrap();
        assert!(!json.contains("prompt"));
        assert!(!json.contains("code_content"));
        let restored = SessionRegistry::restore(serde_json::from_str(&json).unwrap()).unwrap();
        assert_eq!(
            restored.get("s1").unwrap().external_handle_id,
            "shared-handle"
        );

        let mut n_minus_one = serde_json::to_value(registry.snapshot()).unwrap();
        n_minus_one["schema"] = serde_json::json!(SESSION_REGISTRY_SCHEMA_V0);
        let record = n_minus_one["sessions"][0].as_object_mut().unwrap();
        record.remove("resync_reason");
        record.remove("degraded_reason");
        let restored =
            SessionRegistry::restore(serde_json::from_value(n_minus_one).unwrap()).unwrap();
        assert_eq!(restored.get("s1").unwrap().resync_reason, None);
        assert_eq!(restored.get("s1").unwrap().degraded_reason, None);
    }

    #[test]
    fn secrets_paths_controls_and_unknown_payload_fields_are_rejected() {
        for hostile in [
            "Bearer abc",
            "token=abc",
            "/home/user/private",
            "C:\\private",
            "line\nsecret",
        ] {
            let mut value = metadata("p1", "safe");
            value.goal_label = hostile.into();
            assert_eq!(value.validate(), Err(RegistryError::InvalidPayload));
        }
        let mut value = serde_json::to_value(registry().snapshot()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("prompt".into(), serde_json::json!("private"));
        assert!(serde_json::from_value::<RegistrySnapshot>(value).is_err());
    }

    #[test]
    fn unknown_session_and_delayed_response_do_not_create_or_advance_state() {
        let mut registry = registry();
        assert_eq!(
            registry.attach("missing", "tui", 2),
            Err(RegistryError::UnknownSession)
        );
        registry.apply_replay("s1", "host-a", 2, 0, 2).unwrap();
        assert_eq!(
            registry.apply_replay("s1", "host-a", 1, 2, 3),
            Err(RegistryError::DelayedResponse)
        );
        assert_eq!(registry.get("s1").unwrap().cursor, 2);
        assert_eq!(registry.snapshot().sessions.len(), 1);
    }

    proptest::proptest! {
        #[test]
        fn replay_cursor_is_monotonic_and_bounded(steps in proptest::collection::vec(0_u64..32, 1..64)) {
            let mut registry = registry();
            let mut cursor = 0_u64;
            let mut sequence = 0_u64;
            for step in steps {
                sequence += 1;
                let next = cursor + step;
                registry.apply_replay("s1", "host-a", sequence, cursor, next).unwrap();
                prop_assert_eq!(registry.get("s1").unwrap().cursor, next);
                cursor = next;
            }
        }

        #[test]
        fn arbitrary_labels_never_serialize_control_characters(label in ".{0,400}") {
            let mut value = metadata("p1", "safe");
            value.goal_label = label;
            if value.validate().is_ok() {
                let mut registry = SessionRegistry::new();
                registry.create("s".into(), SessionKind::Work, value, "h".into(), "i".into(), 0).unwrap();
                let json = serde_json::to_string(&registry.snapshot()).unwrap();
                prop_assert!(!json.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')));
            }
        }
    }
}
