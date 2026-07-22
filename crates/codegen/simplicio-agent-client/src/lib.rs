//! Fail-closed client for the independent Simplicio Agent host.
//!
//! Simplicio Code requires both Agent and Runtime, but neither product imports
//! Code. This crate owns Code's side of that boundary: it verifies the host's
//! identity, protocol versions, capabilities, and fixed advisory vocabulary
//! before exposing anything to the rest of the application. There is no
//! built-in coordinator or local fallback in this crate.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(windows)]
use std::net::{IpAddr, SocketAddr};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub const HOST_PROTOCOL_SCHEMA: &str = "simplicio.agent-host/v1";
pub const HOST_PROTOCOL_VERSION: u64 = 1;
pub const AGENT_PROTOCOL_VERSION: &str = "agent/v1";
pub const ADVISORY_SCHEMA: &str = "simplicio.agent-advisory/v1";
pub const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 512 * 1024;

const REQUIRED_CAPABILITIES: [&str; 3] = ["host.advisories", "host.status", "turn.start"];
const MAX_ADVISORIES_PER_PAGE: usize = 128;
const MIN_HOST_INSTANCE_ID_BYTES: usize = 16;
const MAX_HOST_INSTANCE_ID_BYTES: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Simplicio Agent socket was not found at {0}; start `simplicio-agent daemon start` or set SIMPLICIO_AGENT_SOCKET"
    )]
    AgentNotFound(PathBuf),
    #[error("Simplicio Agent socket is insecure: {0}")]
    InsecureSocket(String),
    #[error("Simplicio Agent host transport is unsupported on this platform")]
    UnsupportedTransport,
    #[error("Simplicio Agent host I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Simplicio Agent host response: {0}")]
    InvalidResponse(String),
    #[error("invalid Simplicio Agent turn request: {0}")]
    InvalidTurnRequest(String),
    #[error("Simplicio Agent host rejected the operation")]
    OperationRejected,
    #[error("Simplicio Agent host instance identity is invalid")]
    InvalidHostInstanceId,
    #[error("Simplicio Agent host instance identity changed unexpectedly")]
    HostInstanceMismatch,
    #[error("Simplicio Agent advisory cursor was rejected")]
    InvalidAdvisoryCursor,
    #[error("Simplicio Agent host protocol mismatch: {0}")]
    ProtocolMismatch(String),
    #[error("Simplicio Agent host lacks required capabilities: {missing}")]
    CapabilityMismatch { missing: String },
    #[error("invalid coordinator causal identity: {0}")]
    InvalidCausalIdentity(String),
    #[error("coordinator rejected a second active turn")]
    TurnAlreadyActive,
    #[error("coordinator operation is invalid while in state {0:?}")]
    InvalidCoordinatorState(CoordinatorState),
}

/// Opaque, process-lifetime identity for one Agent host incarnation.
///
/// The value is deliberately inaccessible and its debug representation is
/// redacted so it cannot accidentally enter logs or panel state diagnostics.
#[derive(Clone, PartialEq, Eq)]
pub struct HostInstanceId(String);

impl HostInstanceId {
    pub fn from_untrusted(value: &str) -> Result<Self, Error> {
        if !(MIN_HOST_INSTANCE_ID_BYTES..=MAX_HOST_INSTANCE_ID_BYTES).contains(&value.len())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(Error::InvalidHostInstanceId);
        }
        Ok(Self(value.to_owned()))
    }

    fn as_protocol_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for HostInstanceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostInstanceId([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapabilities {
    pub profile: String,
    pub capabilities: BTreeSet<String>,
    host_instance_id: HostInstanceId,
}

impl HostCapabilities {
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn host_instance_id(&self) -> &HostInstanceId {
        &self.host_instance_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdvisorySeverity {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAdvisory {
    pub schema: String,
    pub sequence: u64,
    pub kind: String,
    pub severity: AdvisorySeverity,
    pub summary: String,
    pub action: Option<String>,
    /// Runtime-backed evidence reference. This is an opaque receipt/hash, not
    /// workspace content, so advisory polling cannot exfiltrate source text.
    #[serde(default)]
    pub evidence: Option<String>,
    /// Agent-reported confidence in basis points (0..=10_000).
    #[serde(default)]
    pub confidence_bps: Option<u16>,
    /// Receipt produced by Runtime after an explicitly approved effect.
    #[serde(default)]
    pub receipt_id: Option<String>,
    pub ts_wall_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvisoryPage {
    pub schema: String,
    pub events: Vec<AgentAdvisory>,
    pub next_cursor: u64,
    pub truncated: bool,
}

/// Minimal state a non-focus-stealing side panel can render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttentionState {
    pub cursor: u64,
    pub unread: usize,
    pub highest_severity: Option<AdvisorySeverity>,
    pub latest_summary: Option<String>,
    pub suggested_action: Option<String>,
    pub latest_kind: Option<String>,
    pub latest_evidence: Option<String>,
    pub latest_confidence_bps: Option<u16>,
    pub latest_receipt_id: Option<String>,
    pub history_truncated: bool,
}

impl AdvisoryPage {
    pub fn attention_state(&self) -> AgentAttentionState {
        AgentAttentionState {
            cursor: self.next_cursor,
            unread: self.events.len(),
            highest_severity: self.events.iter().map(|event| event.severity).max(),
            latest_summary: self.events.last().map(|event| event.summary.clone()),
            suggested_action: self.events.last().and_then(|event| event.action.clone()),
            latest_kind: self.events.last().map(|event| event.kind.clone()),
            latest_evidence: self.events.last().and_then(|event| event.evidence.clone()),
            latest_confidence_bps: self.events.last().and_then(|event| event.confidence_bps),
            latest_receipt_id: self
                .events
                .last()
                .and_then(|event| event.receipt_id.clone()),
            history_truncated: self.truncated,
        }
    }
}

/// Causal identity carried across Code, AgentHost, and Runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalIdentity {
    pub workspace_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub attempt_id: String,
    pub idempotency_key: String,
    pub run_id: String,
    pub stage_id: String,
    pub fence: String,
    pub policy_revision: u64,
}

impl CausalIdentity {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Result<Self, Error> {
        let idempotency_key = idempotency_key.into();
        let identity = Self {
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            turn_id: idempotency_key.clone(),
            attempt_id: "0".into(),
            idempotency_key: idempotency_key.clone(),
            run_id: idempotency_key,
            stage_id: "conversation".into(),
            fence: "0".into(),
            policy_revision: 0,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), Error> {
        for (field, value) in [
            ("workspace_id", &self.workspace_id),
            ("session_id", &self.session_id),
            ("turn_id", &self.turn_id),
            ("attempt_id", &self.attempt_id),
            ("idempotency_key", &self.idempotency_key),
            ("run_id", &self.run_id),
            ("stage_id", &self.stage_id),
            ("fence", &self.fence),
        ] {
            if value.trim().is_empty() {
                return Err(Error::InvalidCausalIdentity(format!("{field} is required")));
            }
            if value.len() > 256
                || value
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
            {
                return Err(Error::InvalidCausalIdentity(format!(
                    "{field} contains unsupported characters"
                )));
            }
        }
        if self.turn_id != self.idempotency_key {
            return Err(Error::InvalidCausalIdentity(
                "turn_id must equal idempotency_key".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorState {
    Disconnected,
    Ready,
    Running,
    AwaitingApproval,
    Cancelled,
    Completed,
    EffectUnknown,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorSnapshot {
    pub schema: String,
    pub profile: String,
    pub state: CoordinatorState,
    pub cursor: u64,
    pub active_turn_id: Option<String>,
}

pub const COORDINATOR_SNAPSHOT_SCHEMA: &str = "simplicio.code-coordinator-snapshot/v1";

/// Immutable, validated request for one AgentHost turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentTurnRequest {
    pub profile: String,
    pub workspace_id: String,
    pub session_id: String,
    pub user_message: String,
    pub idempotency_key: String,
    pub turn_id: Option<String>,
    pub attempt_id: String,
    pub incarnation: String,
    pub revision: u64,
    pub run_id: String,
    pub stage_id: String,
    pub fence: String,
}
impl AgentTurnRequest {
    pub fn new(
        profile: impl Into<String>,
        session_id: impl Into<String>,
        user_message: impl Into<String>,
        idempotency_key: impl Into<String>,
    ) -> Result<Self, Error> {
        let idempotency_key = idempotency_key.into();
        let request = Self {
            profile: profile.into(),
            workspace_id: "code".into(),
            session_id: session_id.into(),
            user_message: user_message.into(),
            // The client-side idempotency key is also the stable turn
            // identity. A retry therefore addresses the same lifecycle entry
            // instead of manufacturing a second cancellable turn.
            turn_id: Some(idempotency_key.clone()),
            idempotency_key: idempotency_key.clone(),
            attempt_id: "0".into(),
            incarnation: "default".into(),
            revision: 0,
            run_id: idempotency_key.clone(),
            stage_id: "conversation".into(),
            fence: "0".into(),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn with_identity(
        profile: impl Into<String>,
        identity: &CausalIdentity,
        user_message: impl Into<String>,
    ) -> Result<Self, Error> {
        identity.validate()?;
        let request = Self {
            profile: profile.into(),
            workspace_id: identity.workspace_id.clone(),
            session_id: identity.session_id.clone(),
            user_message: user_message.into(),
            idempotency_key: identity.idempotency_key.clone(),
            turn_id: Some(identity.turn_id.clone()),
            attempt_id: identity.attempt_id.clone(),
            incarnation: identity.run_id.clone(),
            revision: identity.policy_revision,
            run_id: identity.run_id.clone(),
            stage_id: identity.stage_id.clone(),
            fence: identity.fence.clone(),
        };
        request.validate()?;
        Ok(request)
    }
    fn validate(&self) -> Result<(), Error> {
        for (field, value) in [
            ("profile", &self.profile),
            ("workspace_id", &self.workspace_id),
            ("session_id", &self.session_id),
            ("user_message", &self.user_message),
            ("idempotency_key", &self.idempotency_key),
            ("attempt_id", &self.attempt_id),
            ("incarnation", &self.incarnation),
            ("run_id", &self.run_id),
            ("stage_id", &self.stage_id),
            ("fence", &self.fence),
        ] {
            if value.trim().is_empty() {
                return Err(Error::InvalidTurnRequest(format!("{field} is required")));
            }
        }
        if self
            .turn_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(Error::InvalidTurnRequest(
                "turn_id must be non-empty when provided".into(),
            ));
        }
        Ok(())
    }
}
/// Sanitized terminal result returned by a completed AgentHost turn.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentTurnResult {
    #[serde(default)]
    pub final_response: Option<String>,
    #[serde(default)]
    pub messages: Vec<Value>,
    #[serde(default)]
    pub api_calls: u64,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Truthful outcome of a cancellation request. `Running` never means the
/// operation stopped: callers must wait for or replay its terminal receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTurnCancelOutcome {
    Cancelled,
    Running,
    Terminal,
    NotFound,
}
#[derive(Debug)]
pub struct AgentHostClient {
    socket_path: PathBuf,
    capabilities: HostCapabilities,
}

impl AgentHostClient {
    pub fn connect_default() -> Result<Self, Error> {
        Self::connect(resolve_socket_path())
    }

    pub fn connect(socket_path: impl Into<PathBuf>) -> Result<Self, Error> {
        let socket_path = socket_path.into();
        let response = request(&socket_path, &json!({ "op": "host.status" }))?;
        let capabilities = validate_ready_host_response(&response, None)?;
        Ok(Self {
            socket_path,
            capabilities,
        })
    }

    pub fn capabilities(&self) -> &HostCapabilities {
        &self.capabilities
    }

    /// Re-check a cached client's liveness and compatibility before use.
    pub fn refresh_status(&mut self) -> Result<&HostCapabilities, Error> {
        let response = request(&self.socket_path, &json!({ "op": "host.status" }))?;
        // Status is discovery: a restarted daemon legitimately returns a new
        // incarnation. Callers bind any cursor to the returned identity.
        self.capabilities = validate_ready_host_response(&response, None)?;
        Ok(&self.capabilities)
    }

    /// Submit one idempotent turn through the already negotiated AgentHost.
    pub fn start_turn(&self, turn: &AgentTurnRequest) -> Result<AgentTurnResult, Error> {
        turn.validate()?;
        let response = request(
            &self.socket_path,
            &json!({
                "op": "turn.start",
                "host_instance_id": self.capabilities.host_instance_id.as_protocol_str(),
                "profile": turn.profile,
                "workspace_id": turn.workspace_id,
                "session_id": turn.session_id,
                "message": turn.user_message,
                "idempotency_key": turn.idempotency_key,
                "turn_id": turn.turn_id,
                "attempt_id": turn.attempt_id,
                "incarnation": turn.incarnation,
                "revision": turn.revision,
                "run_id": turn.run_id,
                "stage_id": turn.stage_id,
                "fence": turn.fence,
            }),
        )?;
        validate_host_response(&response, Some(self.capabilities.host_instance_id()))?;
        parse_turn_result(&response)
    }

    /// Requests cancellation for a causally identified turn when the host
    /// explicitly advertises `turn.cancel`.
    pub fn cancel_turn(&self, turn_id: &str) -> Result<AgentTurnCancelOutcome, Error> {
        if turn_id.trim().is_empty() {
            return Err(Error::InvalidTurnRequest("turn_id is required".into()));
        }
        if !self.capabilities.supports("turn.cancel") {
            return Err(Error::CapabilityMismatch {
                missing: "turn.cancel".into(),
            });
        }
        let response = request(
            &self.socket_path,
            &json!({
                "op": "turn.cancel",
                "turn_id": turn_id,
                "host_instance_id": self.capabilities.host_instance_id.as_protocol_str(),
            }),
        )?;
        validate_host_response(&response, Some(self.capabilities.host_instance_id()))?;
        match response.get("status").and_then(Value::as_str) {
            Some("cancelled") => Ok(AgentTurnCancelOutcome::Cancelled),
            Some("running") => Ok(AgentTurnCancelOutcome::Running),
            Some("terminal") => Ok(AgentTurnCancelOutcome::Terminal),
            Some("not_found") => Ok(AgentTurnCancelOutcome::NotFound),
            _ => Err(Error::InvalidResponse("turn.cancel missing status".into())),
        }
    }

    /// Replay fixed, generic host signals for a passive side-panel projection.
    pub fn advisories(&self, after: u64) -> Result<AdvisoryPage, Error> {
        let response = request(
            &self.socket_path,
            &json!({
                "op": "host.advisories",
                "cursor": after,
                "host_instance_id": self.capabilities.host_instance_id.as_protocol_str(),
            }),
        )?;
        match validate_host_response(&response, Some(self.capabilities.host_instance_id())) {
            Ok(_) => {}
            Err(Error::OperationRejected) => return Err(Error::InvalidAdvisoryCursor),
            Err(error) => return Err(error),
        }
        parse_advisory_page(&response, after, self.capabilities.host_instance_id())
    }
}

/// Stateful Code-side coordinator adapter for one AgentHost incarnation.
///
/// It serializes turns, keeps causal identity stable across retries, and
/// treats a reconnect before reconciliation as effect_unknown. Runtime
/// effects remain outside this type and use the Runtime-backed tool boundary.
#[derive(Debug)]
pub struct AgentHostCoordinator {
    profile: String,
    client: AgentHostClient,
    state: CoordinatorState,
    cursor: u64,
    active_turn_id: Option<String>,
}

impl AgentHostCoordinator {
    pub fn connect(profile: impl Into<String>) -> Result<Self, Error> {
        Self::connect_at(profile, resolve_socket_path())
    }

    pub fn connect_at(
        profile: impl Into<String>,
        socket_path: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let profile = profile.into();
        let client = AgentHostClient::connect(socket_path)?;
        Self::from_client(profile, client)
    }

    pub fn from_client(profile: impl Into<String>, client: AgentHostClient) -> Result<Self, Error> {
        let profile = profile.into();
        if client.capabilities().profile != profile {
            return Err(Error::ProtocolMismatch(format!(
                "AgentHost profile '{}', expected '{profile}'",
                client.capabilities().profile
            )));
        }
        Ok(Self {
            profile,
            client,
            state: CoordinatorState::Ready,
            cursor: 0,
            active_turn_id: None,
        })
    }

    pub fn ensure_ready(&mut self) -> Result<(), Error> {
        let expected = self.client.capabilities().profile.clone();
        let before = self.client.capabilities().clone();
        match self.client.refresh_status() {
            Ok(capabilities) if capabilities.profile == expected => {
                let changed = before != capabilities.clone();
                if changed && self.active_turn_id.is_some() {
                    self.state = CoordinatorState::EffectUnknown;
                } else if self.state == CoordinatorState::Disconnected {
                    self.state = CoordinatorState::Ready;
                }
                Ok(())
            }
            Ok(capabilities) => Err(Error::ProtocolMismatch(format!(
                "AgentHost profile '{}', expected '{expected}'",
                capabilities.profile
            ))),
            Err(error) => {
                self.state = CoordinatorState::Disconnected;
                Err(error)
            }
        }
    }

    pub fn start_turn(
        &mut self,
        identity: &CausalIdentity,
        message: impl Into<String>,
    ) -> Result<AgentTurnResult, Error> {
        identity.validate()?;
        if self.active_turn_id.is_some()
            || matches!(
                self.state,
                CoordinatorState::Running | CoordinatorState::AwaitingApproval
            )
        {
            return Err(Error::TurnAlreadyActive);
        }
        if self.state == CoordinatorState::Disconnected {
            self.ensure_ready()?;
        }
        self.active_turn_id = Some(identity.turn_id.clone());
        self.state = CoordinatorState::Running;
        let request = AgentTurnRequest::with_identity(&self.profile, identity, message)?;
        let result = match self.client.start_turn(&request) {
            Ok(result) => result,
            Err(error) => {
                self.state = CoordinatorState::EffectUnknown;
                return Err(error);
            }
        };
        self.state = if result.completed {
            CoordinatorState::Completed
        } else if result.interrupted {
            CoordinatorState::Cancelled
        } else {
            CoordinatorState::Terminal
        };
        self.active_turn_id = None;
        Ok(result)
    }

    pub fn cancel_turn(&mut self, turn_id: &str) -> Result<AgentTurnCancelOutcome, Error> {
        if self.active_turn_id.as_deref() != Some(turn_id) {
            return Err(Error::InvalidCoordinatorState(self.state));
        }
        let outcome = self.client.cancel_turn(turn_id)?;
        self.state = match outcome {
            AgentTurnCancelOutcome::Cancelled => CoordinatorState::Cancelled,
            AgentTurnCancelOutcome::Running => CoordinatorState::Running,
            AgentTurnCancelOutcome::Terminal => CoordinatorState::Terminal,
            AgentTurnCancelOutcome::NotFound => CoordinatorState::EffectUnknown,
        };
        if !matches!(outcome, AgentTurnCancelOutcome::Running) {
            self.active_turn_id = None;
        }
        Ok(outcome)
    }

    pub fn reconnect(&mut self) -> Result<CoordinatorSnapshot, Error> {
        let before = self.client.capabilities().clone();
        if let Err(error) = self.client.refresh_status() {
            self.state = CoordinatorState::Disconnected;
            return Err(error);
        }
        if before != self.client.capabilities().clone() {
            self.cursor = 0;
            if self.active_turn_id.is_some() {
                self.state = CoordinatorState::EffectUnknown;
            }
        }
        if self.state == CoordinatorState::Disconnected {
            self.state = CoordinatorState::Ready;
        }
        Ok(self.snapshot())
    }

    pub fn replay(&mut self, after: Option<u64>) -> Result<AdvisoryPage, Error> {
        let cursor = after.unwrap_or(self.cursor);
        let page = self.client.advisories(cursor)?;
        self.cursor = page.next_cursor;
        Ok(page)
    }

    pub fn snapshot(&self) -> CoordinatorSnapshot {
        CoordinatorSnapshot {
            schema: COORDINATOR_SNAPSHOT_SCHEMA.into(),
            profile: self.profile.clone(),
            state: self.state,
            cursor: self.cursor,
            active_turn_id: self.active_turn_id.clone(),
        }
    }

    pub fn state(&self) -> CoordinatorState {
        self.state
    }

    pub fn active_turn_id(&self) -> Option<&str> {
        self.active_turn_id.as_deref()
    }
}

/// Resolve the socket with explicit Simplicio overrides first while retaining
/// the Agent's current standalone default (`~/.hermes/daemon.sock`).
pub fn resolve_socket_path() -> PathBuf {
    if let Some(path) = non_empty_env("SIMPLICIO_AGENT_SOCKET") {
        return PathBuf::from(path);
    }
    if let Some(home) = non_empty_env("SIMPLICIO_AGENT_HOME") {
        return PathBuf::from(home).join("daemon.sock");
    }
    if let Some(home) = non_empty_env("HERMES_HOME") {
        return PathBuf::from(home).join("daemon.sock");
    }
    if let Some(home) = non_empty_env("HOME") {
        return PathBuf::from(home).join(".hermes/daemon.sock");
    }
    PathBuf::from(".hermes/daemon.sock")
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(windows)]
fn parse_loopback_endpoint(value: &str) -> Result<SocketAddr, Error> {
    let endpoint = value
        .trim()
        .parse::<SocketAddr>()
        .map_err(|_| Error::InvalidResponse("Agent loopback endpoint is invalid".into()))?;
    if !matches!(endpoint.ip(), IpAddr::V4(ip) if ip.is_loopback()) {
        return Err(Error::InvalidResponse(
            "Agent endpoint must be an IPv4 loopback address".into(),
        ));
    }
    Ok(endpoint)
}

#[cfg(windows)]
fn windows_loopback_transport(socket_path: &Path) -> Result<(SocketAddr, String), Error> {
    let endpoint_path = socket_path.with_extension("tcp");
    let endpoint_raw =
        std::fs::read_to_string(&endpoint_path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::AgentNotFound(endpoint_path),
            _ => Error::Io(error),
        })?;
    let token_path = socket_path.with_extension("token");
    let token = std::fs::read_to_string(&token_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Error::AgentNotFound(token_path),
        _ => Error::Io(error),
    })?;
    let token = token.trim().to_owned();
    if !(32..=256).contains(&token.len())
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(Error::InvalidResponse(
            "Agent loopback token is invalid".into(),
        ));
    }
    Ok((parse_loopback_endpoint(&endpoint_raw)?, token))
}

#[derive(Debug, Deserialize)]
struct HostEnvelope {
    ok: bool,
    #[serde(default)]
    host_instance_id: Option<Value>,
    protocol_schema: String,
    protocol_version: u64,
    agent_protocol: String,
    profile: String,
    capabilities: BTreeSet<String>,
    advisory_schema: String,
}

fn parse_host_instance_id(value: Option<&Value>) -> Result<HostInstanceId, Error> {
    let Some(Value::String(value)) = value else {
        return Err(Error::InvalidHostInstanceId);
    };
    HostInstanceId::from_untrusted(value)
}

fn validate_host_response(
    response: &Value,
    expected_host_instance_id: Option<&HostInstanceId>,
) -> Result<HostCapabilities, Error> {
    let envelope: HostEnvelope = serde_json::from_value(response.clone())
        .map_err(|error| Error::InvalidResponse(error.to_string()))?;
    let host_instance_id = parse_host_instance_id(envelope.host_instance_id.as_ref())?;
    if expected_host_instance_id.is_some_and(|expected| expected != &host_instance_id) {
        return Err(Error::HostInstanceMismatch);
    }
    if envelope.protocol_schema != HOST_PROTOCOL_SCHEMA {
        return Err(Error::ProtocolMismatch(format!(
            "schema '{}', expected '{HOST_PROTOCOL_SCHEMA}'",
            envelope.protocol_schema
        )));
    }
    if envelope.protocol_version != HOST_PROTOCOL_VERSION {
        return Err(Error::ProtocolMismatch(format!(
            "host version {}, expected {HOST_PROTOCOL_VERSION}",
            envelope.protocol_version
        )));
    }
    if envelope.agent_protocol != AGENT_PROTOCOL_VERSION {
        return Err(Error::ProtocolMismatch(format!(
            "agent protocol '{}', expected '{AGENT_PROTOCOL_VERSION}'",
            envelope.agent_protocol
        )));
    }
    if envelope.advisory_schema != ADVISORY_SCHEMA {
        return Err(Error::ProtocolMismatch(format!(
            "advisory schema '{}', expected '{ADVISORY_SCHEMA}'",
            envelope.advisory_schema
        )));
    }
    if envelope.profile.trim().is_empty() {
        return Err(Error::InvalidResponse("profile is empty".into()));
    }
    let missing = REQUIRED_CAPABILITIES
        .iter()
        .filter(|capability| !envelope.capabilities.contains(**capability))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(Error::CapabilityMismatch {
            missing: missing.join(", "),
        });
    }
    if !envelope.ok {
        return Err(Error::OperationRejected);
    }
    Ok(HostCapabilities {
        profile: envelope.profile,
        capabilities: envelope.capabilities,
        host_instance_id,
    })
}

#[derive(Debug, Deserialize)]
struct HostStatus {
    ready: bool,
    stopping: bool,
    #[serde(default)]
    host_instance_id: Option<Value>,
}

fn validate_ready_host_response(
    response: &Value,
    expected_host_instance_id: Option<&HostInstanceId>,
) -> Result<HostCapabilities, Error> {
    let capabilities = validate_host_response(response, expected_host_instance_id)?;
    let status: HostStatus = serde_json::from_value(
        response
            .get("host")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("host status is missing".into()))?,
    )
    .map_err(|error| Error::InvalidResponse(error.to_string()))?;
    let nested_host_instance_id = parse_host_instance_id(status.host_instance_id.as_ref())?;
    if &nested_host_instance_id != capabilities.host_instance_id() {
        return Err(Error::HostInstanceMismatch);
    }
    if !status.ready || status.stopping {
        return Err(Error::OperationRejected);
    }
    Ok(capabilities)
}

#[derive(Debug, Deserialize)]
struct AdvisoryPageEnvelope {
    #[serde(default)]
    host_instance_id: Option<Value>,
    schema: String,
    events: Vec<AgentAdvisory>,
    next_cursor: u64,
    truncated: bool,
}

fn parse_turn_result(response: &Value) -> Result<AgentTurnResult, Error> {
    let result: AgentTurnResult = serde_json::from_value(
        response
            .get("result")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("turn result is missing".into()))?,
    )
    .map_err(|error| Error::InvalidResponse(error.to_string()))?;
    let terminal_count = [result.completed, result.failed, result.interrupted]
        .into_iter()
        .filter(|state| *state)
        .count();
    if terminal_count != 1 {
        return Err(Error::InvalidResponse(
            "turn result must contain exactly one terminal state".into(),
        ));
    }
    if result.completed && result.error.is_some() {
        return Err(Error::InvalidResponse(
            "completed turn result must not carry an error".into(),
        ));
    }
    Ok(result)
}

fn parse_advisory_page(
    response: &Value,
    after: u64,
    expected_host_instance_id: &HostInstanceId,
) -> Result<AdvisoryPage, Error> {
    let page: AdvisoryPageEnvelope = serde_json::from_value(
        response
            .get("advisories")
            .cloned()
            .ok_or_else(|| Error::InvalidResponse("advisories field is missing".into()))?,
    )
    .map_err(|error| Error::InvalidResponse(error.to_string()))?;
    let page_host_instance_id = parse_host_instance_id(page.host_instance_id.as_ref())?;
    if &page_host_instance_id != expected_host_instance_id {
        return Err(Error::HostInstanceMismatch);
    }
    if page.schema != ADVISORY_SCHEMA {
        return Err(Error::ProtocolMismatch(format!(
            "advisory page schema '{}', expected '{ADVISORY_SCHEMA}'",
            page.schema
        )));
    }
    if page.events.len() > MAX_ADVISORIES_PER_PAGE {
        return Err(Error::InvalidResponse(format!(
            "advisory page exceeds {MAX_ADVISORIES_PER_PAGE} events"
        )));
    }
    let mut previous = after;
    for (index, event) in page.events.iter().enumerate() {
        if event.schema != ADVISORY_SCHEMA {
            return Err(Error::ProtocolMismatch(format!(
                "event schema '{}', expected '{ADVISORY_SCHEMA}'",
                event.schema
            )));
        }
        if event.sequence <= previous {
            return Err(Error::InvalidResponse(
                "advisory sequences must be unique, increasing, and after the cursor".into(),
            ));
        }
        if event.sequence != previous.saturating_add(1) && !(index == 0 && page.truncated) {
            return Err(Error::InvalidResponse(
                "advisory sequences must be contiguous unless history was truncated".into(),
            ));
        }
        validate_advisory(event)?;
        previous = event.sequence;
    }
    if page.next_cursor != previous {
        return Err(Error::InvalidResponse(
            "advisory next_cursor must equal the last observed sequence".into(),
        ));
    }
    Ok(AdvisoryPage {
        schema: page.schema,
        events: page.events,
        next_cursor: page.next_cursor,
        truncated: page.truncated,
    })
}

fn validate_advisory(event: &AgentAdvisory) -> Result<(), Error> {
    validate_projection_text("summary", &event.summary, 512)?;
    for (name, value) in [
        ("action", event.action.as_deref()),
        ("evidence", event.evidence.as_deref()),
        ("receipt_id", event.receipt_id.as_deref()),
    ] {
        if let Some(value) = value {
            validate_projection_text(name, value, 512)?;
        }
    }
    if event.confidence_bps.is_some_and(|value| value > 10_000) {
        return Err(Error::InvalidResponse(
            "advisory confidence exceeds 100%".into(),
        ));
    }

    // Proactive advisories carry bounded presentation data and opaque Runtime
    // evidence only. Suggested actions remain inert until a separate approval.
    if matches!(
        event.kind.as_str(),
        "attention" | "finding" | "risk" | "suggestion" | "plan" | "progress" | "receipt"
    ) {
        if matches!(event.kind.as_str(), "finding" | "risk" | "suggestion")
            && (event.evidence.is_none() || event.confidence_bps.is_none())
        {
            return Err(Error::InvalidResponse(format!(
                "proactive advisory '{}' requires evidence and confidence",
                event.kind
            )));
        }
        if event.kind == "receipt" && event.receipt_id.is_none() {
            return Err(Error::InvalidResponse(
                "receipt advisory requires receipt_id".into(),
            ));
        }
        return Ok(());
    }
    let expected = match event.kind.as_str() {
        "host.ready" => (AdvisorySeverity::Info, "Agent host is ready.", None),
        "host.backpressure" => (
            AdvisorySeverity::Warning,
            "Agent host is saturated.",
            Some("retry"),
        ),
        "host.draining" => (AdvisorySeverity::Warning, "Agent host is draining.", None),
        "turn.completed" => (AdvisorySeverity::Info, "Agent turn completed.", None),
        "turn.failed" => (
            AdvisorySeverity::Warning,
            "Agent turn failed.",
            Some("inspect_logs"),
        ),
        unknown => {
            return Err(Error::InvalidResponse(format!(
                "unknown advisory kind '{unknown}'"
            )));
        }
    };
    if event.severity != expected.0
        || event.summary != expected.1
        || event.action.as_deref() != expected.2
    {
        return Err(Error::InvalidResponse(format!(
            "advisory '{}' does not match its fixed catalog entry",
            event.kind
        )));
    }
    Ok(())
}

fn validate_projection_text(name: &str, value: &str, max_bytes: usize) -> Result<(), Error> {
    if value.trim().is_empty()
        || value.len() > max_bytes
        || value.chars().any(|character| character.is_control())
    {
        return Err(Error::InvalidResponse(format!(
            "advisory {name} is not safe for projection"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn request(socket_path: &Path, payload: &Value) -> Result<Value, Error> {
    use std::{
        io::{Read, Write},
        net::Shutdown,
        os::unix::{
            fs::{FileTypeExt, PermissionsExt},
            net::UnixStream,
        },
        time::Duration,
    };

    let metadata = std::fs::symlink_metadata(socket_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => Error::AgentNotFound(socket_path.to_path_buf()),
        _ => Error::Io(error),
    })?;
    if !metadata.file_type().is_socket() {
        return Err(Error::InsecureSocket(format!(
            "{} is not a Unix socket",
            socket_path.display()
        )));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error::InsecureSocket(format!(
            "{} must not grant group/other permissions",
            socket_path.display()
        )));
    }

    let mut stream = UnixStream::connect(socket_path)?;
    let timeout = Some(Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let encoded =
        serde_json::to_vec(payload).map_err(|error| Error::InvalidResponse(error.to_string()))?;
    stream.write_all(&encoded)?;
    stream.shutdown(Shutdown::Write)?;

    let mut bytes = Vec::new();
    stream
        .take((DEFAULT_MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > DEFAULT_MAX_RESPONSE_BYTES {
        return Err(Error::InvalidResponse(format!(
            "response exceeds {DEFAULT_MAX_RESPONSE_BYTES} bytes"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| Error::InvalidResponse(error.to_string()))
}

#[cfg(not(unix))]
#[cfg(not(windows))]
fn request(_socket_path: &Path, _payload: &Value) -> Result<Value, Error> {
    Err(Error::UnsupportedTransport)
}

#[cfg(windows)]
fn request(socket_path: &Path, payload: &Value) -> Result<Value, Error> {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };

    let (endpoint, token) = windows_loopback_transport(socket_path)?;
    let mut payload = payload.clone();
    let object = payload
        .as_object_mut()
        .ok_or_else(|| Error::InvalidResponse("Agent request must be an object".into()))?;
    object.insert("auth_token".into(), Value::String(token));

    let timeout = Duration::from_millis(DEFAULT_REQUEST_TIMEOUT_MS);
    let mut stream = TcpStream::connect_timeout(&endpoint, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let encoded =
        serde_json::to_vec(&payload).map_err(|error| Error::InvalidResponse(error.to_string()))?;
    stream.write_all(&encoded)?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut bytes = Vec::new();
    stream
        .take((DEFAULT_MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > DEFAULT_MAX_RESPONSE_BYTES {
        return Err(Error::InvalidResponse(format!(
            "response exceeds {DEFAULT_MAX_RESPONSE_BYTES} bytes"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| Error::InvalidResponse(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_ID: &str = "agent-host-instance_1234567890";

    fn host_instance_id() -> HostInstanceId {
        HostInstanceId::from_untrusted(HOST_ID).unwrap()
    }

    fn host_response() -> Value {
        json!({
            "ok": true,
            "host_instance_id": HOST_ID,
            "protocol_schema": HOST_PROTOCOL_SCHEMA,
            "protocol_version": HOST_PROTOCOL_VERSION,
            "agent_protocol": AGENT_PROTOCOL_VERSION,
            "profile": "desktop",
            "capabilities": ["host.advisories", "host.status", "turn.start"],
            "advisory_schema": ADVISORY_SCHEMA,
            "host": {
                "ready": true,
                "stopping": false,
                "host_instance_id": HOST_ID,
            },
        })
    }

    #[test]
    fn turn_request_rejects_missing_causal_identity() {
        assert!(matches!(
            AgentTurnRequest::new("desktop", "session", "message", ""),
            Err(Error::InvalidTurnRequest(_))
        ));
        let request = AgentTurnRequest::new("desktop", "session", "message", "key").unwrap();
        assert_eq!(request.turn_id.as_deref(), Some("key"));
    }

    #[test]
    fn causal_identity_is_complete_and_serializable_at_the_turn_boundary() {
        let identity = CausalIdentity::new("workspace-1", "session-1", "turn-1").unwrap();
        let request = AgentTurnRequest::with_identity("desktop", &identity, "inspect").unwrap();
        let wire = serde_json::to_value(request).unwrap();
        for field in [
            "workspace_id",
            "session_id",
            "turn_id",
            "attempt_id",
            "idempotency_key",
            "run_id",
            "stage_id",
            "fence",
        ] {
            assert!(wire.get(field).is_some(), "missing causal field {field}");
        }
    }

    #[test]
    fn coordinator_snapshot_has_explicit_effect_unknown_state() {
        let snapshot = CoordinatorSnapshot {
            schema: COORDINATOR_SNAPSHOT_SCHEMA.into(),
            profile: "desktop".into(),
            state: CoordinatorState::EffectUnknown,
            cursor: 7,
            active_turn_id: Some("turn-1".into()),
        };
        let wire = serde_json::to_value(snapshot).unwrap();
        assert_eq!(wire["state"], "effect_unknown");
        assert_eq!(wire["cursor"], 7);
    }
    #[test]
    fn turn_result_requires_one_terminal_state() {
        let mut response = host_response();
        response["result"] = json!({"final_response":"done","api_calls":1,"completed":true,"failed":false,"interrupted":false});
        assert_eq!(
            parse_turn_result(&response)
                .unwrap()
                .final_response
                .as_deref(),
            Some("done")
        );
        response["result"]["failed"] = json!(true);
        assert!(matches!(
            parse_turn_result(&response),
            Err(Error::InvalidResponse(_))
        ));
    }

    #[test]
    fn accepts_exact_agent_host_contract() {
        let capabilities = validate_ready_host_response(&host_response(), None).unwrap();

        assert_eq!(capabilities.profile, "desktop");
        assert!(capabilities.supports("host.advisories"));
        assert_eq!(capabilities.host_instance_id(), &host_instance_id());
    }

    #[test]
    fn causal_identity_requires_stable_turn_and_idempotency_identity() {
        let mut identity = CausalIdentity::new("workspace-1", "session-1", "turn-1").unwrap();
        identity.turn_id = "different-turn".into();
        assert!(matches!(
            identity.validate(),
            Err(Error::InvalidCausalIdentity(message))
                if message == "turn_id must equal idempotency_key"
        ));
    }

    #[test]
    fn fails_closed_on_version_or_capability_mismatch() {
        let mut wrong_version = host_response();
        wrong_version["protocol_version"] = json!(2);
        assert!(matches!(
            validate_host_response(&wrong_version, None),
            Err(Error::ProtocolMismatch(_))
        ));

        let mut missing_capability = host_response();
        missing_capability["capabilities"] = json!(["host.status", "turn.start"]);
        assert!(matches!(
            validate_host_response(&missing_capability, None),
            Err(Error::CapabilityMismatch { .. })
        ));
    }

    #[test]
    fn fails_closed_when_agent_host_is_not_ready() {
        let mut response = host_response();
        response["host"]["stopping"] = json!(true);

        assert!(matches!(
            validate_ready_host_response(&response, None),
            Err(Error::OperationRejected)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_regular_file_before_attempting_transport() {
        let file = tempfile::NamedTempFile::new().unwrap();

        assert!(matches!(
            AgentHostClient::connect(file.path()),
            Err(Error::InsecureSocket(_))
        ));
    }

    #[test]
    fn projects_valid_advisories_for_a_passive_panel() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [{
                "schema": ADVISORY_SCHEMA,
                "sequence": 1,
                "kind": "host.backpressure",
                "severity": "warning",
                "summary": "Agent host is saturated.",
                "action": "retry",
                "ts_wall_ns": 1,
            }],
            "next_cursor": 1,
            "truncated": false,
        });

        let page = parse_advisory_page(&response, 0, &host_instance_id()).unwrap();
        assert_eq!(
            page.attention_state(),
            AgentAttentionState {
                cursor: 1,
                unread: 1,
                highest_severity: Some(AdvisorySeverity::Warning),
                latest_summary: Some("Agent host is saturated.".into()),
                suggested_action: Some("retry".into()),
                latest_kind: Some("host.backpressure".into()),
                latest_evidence: None,
                latest_confidence_bps: None,
                latest_receipt_id: None,
                history_truncated: false,
            }
        );
    }

    #[test]
    fn projects_proactive_finding_with_evidence_and_confidence() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [{
                "schema": ADVISORY_SCHEMA,
                "sequence": 1,
                "kind": "finding",
                "severity": "warning",
                "summary": "Validation coverage regressed.",
                "action": "review_validation",
                "evidence": "runtime://receipt/test-42",
                "confidence_bps": 9750,
                "ts_wall_ns": 1
            }],
            "next_cursor": 1,
            "truncated": false
        });

        let attention = parse_advisory_page(&response, 0, &host_instance_id())
            .unwrap()
            .attention_state();
        assert_eq!(attention.latest_kind.as_deref(), Some("finding"));
        assert_eq!(
            attention.latest_evidence.as_deref(),
            Some("runtime://receipt/test-42")
        );
        assert_eq!(attention.latest_confidence_bps, Some(9750));
        assert_eq!(
            attention.suggested_action.as_deref(),
            Some("review_validation")
        );
    }

    #[test]
    fn rejects_unsubstantiated_or_unbounded_proactive_advisories() {
        let base = AgentAdvisory {
            schema: ADVISORY_SCHEMA.into(),
            sequence: 1,
            kind: "risk".into(),
            severity: AdvisorySeverity::Warning,
            summary: "Possible regression.".into(),
            action: None,
            evidence: None,
            confidence_bps: Some(10_001),
            receipt_id: None,
            ts_wall_ns: 1,
        };
        assert!(validate_advisory(&base).is_err());
        let mut oversized = base;
        oversized.confidence_bps = Some(5000);
        oversized.evidence = Some("x".repeat(513));
        assert!(validate_advisory(&oversized).is_err());
    }

    #[test]
    #[ignore = "microbenchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_proactive_advisory_validation() {
        let advisory = AgentAdvisory {
            schema: ADVISORY_SCHEMA.into(),
            sequence: 1,
            kind: "finding".into(),
            severity: AdvisorySeverity::Warning,
            summary: "Validation coverage regressed.".into(),
            action: Some("review_validation".into()),
            evidence: Some("runtime://receipt/test-42".into()),
            confidence_bps: Some(9750),
            receipt_id: None,
            ts_wall_ns: 1,
        };
        let iterations = 100_000u32;
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(validate_advisory(std::hint::black_box(&advisory))).unwrap();
        }
        let elapsed = started.elapsed();
        eprintln!(
            "proactive_advisory_validation iterations={iterations} elapsed_ns={} ns_per_op={:.2}",
            elapsed.as_nanos(),
            elapsed.as_nanos() as f64 / f64::from(iterations)
        );
    }

    #[test]
    fn rejects_free_form_or_replayed_advisory_content() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [{
                "schema": ADVISORY_SCHEMA,
                "sequence": 7,
                "kind": "host.ready",
                "severity": "info",
                "summary": "workspace prompt or secret",
                "action": null,
                "ts_wall_ns": 1,
            }],
            "next_cursor": 7,
            "truncated": false,
        });
        assert!(matches!(
            parse_advisory_page(&response, 6, &host_instance_id()),
            Err(Error::InvalidResponse(_))
        ));

        response["advisories"]["events"][0]["summary"] = json!("Agent host is ready.");
        assert!(matches!(
            parse_advisory_page(&response, 7, &host_instance_id()),
            Err(Error::InvalidResponse(_))
        ));
    }

    #[test]
    fn rejects_a_future_cursor_that_would_skip_advisories() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [{
                "schema": ADVISORY_SCHEMA,
                "sequence": 8,
                "kind": "host.ready",
                "severity": "info",
                "summary": "Agent host is ready.",
                "action": null,
                "ts_wall_ns": 1,
            }],
            "next_cursor": 99,
            "truncated": false,
        });

        assert!(matches!(
            parse_advisory_page(&response, 7, &host_instance_id()),
            Err(Error::InvalidResponse(_))
        ));
    }

    #[test]
    fn accepts_only_the_exact_bounded_opaque_instance_id_alphabet() {
        assert!(HostInstanceId::from_untrusted(&"a".repeat(16)).is_ok());
        assert!(HostInstanceId::from_untrusted(&"Z_9-".repeat(16)).is_ok());

        for invalid in [
            "a".repeat(15),
            "a".repeat(65),
            "valid-length-but!".to_owned(),
            "validlengthbuté".to_owned(),
        ] {
            assert!(matches!(
                HostInstanceId::from_untrusted(&invalid),
                Err(Error::InvalidHostInstanceId)
            ));
        }
    }

    #[test]
    fn missing_null_malformed_or_mismatched_instance_ids_fail_closed() {
        let cases = [
            Value::Null,
            json!("too-short"),
            json!("x".repeat(65)),
            json!("invalid instance id"),
            json!(17),
        ];
        for invalid in cases {
            let mut response = host_response();
            response["host_instance_id"] = invalid;
            assert!(matches!(
                validate_ready_host_response(&response, None),
                Err(Error::InvalidHostInstanceId)
            ));
        }

        let mut missing = host_response();
        missing.as_object_mut().unwrap().remove("host_instance_id");
        assert!(matches!(
            validate_ready_host_response(&missing, None),
            Err(Error::InvalidHostInstanceId)
        ));

        let other = "other-host-instance_1234567890";
        let mut nested_mismatch = host_response();
        nested_mismatch["host"]["host_instance_id"] = json!(other);
        assert!(matches!(
            validate_ready_host_response(&nested_mismatch, None),
            Err(Error::HostInstanceMismatch)
        ));

        let expected = HostInstanceId::from_untrusted(other).unwrap();
        assert!(matches!(
            validate_host_response(&host_response(), Some(&expected)),
            Err(Error::HostInstanceMismatch)
        ));
    }

    #[test]
    fn advisory_page_requires_the_expected_instance_at_both_levels() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [],
            "next_cursor": 0,
            "truncated": false,
        });
        assert!(parse_advisory_page(&response, 0, &host_instance_id()).is_ok());

        response["advisories"]["host_instance_id"] = json!("other-host-instance_1234567890");
        assert!(matches!(
            parse_advisory_page(&response, 0, &host_instance_id()),
            Err(Error::HostInstanceMismatch)
        ));

        response["advisories"]
            .as_object_mut()
            .unwrap()
            .remove("host_instance_id");
        assert!(matches!(
            parse_advisory_page(&response, 0, &host_instance_id()),
            Err(Error::InvalidHostInstanceId)
        ));
    }

    #[test]
    fn instance_identity_and_rejection_details_are_redacted() {
        let id = host_instance_id();
        let debug = format!("{id:?}");
        assert_eq!(debug, "HostInstanceId([redacted])");
        assert!(!debug.contains(HOST_ID));

        let mut response = host_response();
        response["ok"] = json!(false);
        response["error"] = json!(format!("host instance {HOST_ID} was rejected"));
        let error = validate_host_response(&response, None).unwrap_err();
        assert!(matches!(&error, Error::OperationRejected));
        assert!(!format!("{error:?}").contains(HOST_ID));
        assert!(!error.to_string().contains(HOST_ID));
    }

    #[test]
    fn gaps_require_an_explicit_truncation_marker() {
        let mut response = host_response();
        response["advisories"] = json!({
            "host_instance_id": HOST_ID,
            "schema": ADVISORY_SCHEMA,
            "events": [{
                "schema": ADVISORY_SCHEMA,
                "sequence": 9,
                "kind": "host.ready",
                "severity": "info",
                "summary": "Agent host is ready.",
                "action": null,
                "ts_wall_ns": 1,
            }],
            "next_cursor": 9,
            "truncated": false,
        });
        assert!(matches!(
            parse_advisory_page(&response, 0, &host_instance_id()),
            Err(Error::InvalidResponse(_))
        ));

        response["advisories"]["truncated"] = json!(true);
        assert!(parse_advisory_page(&response, 0, &host_instance_id()).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn windows_loopback_transport_rejects_non_loopback_endpoints() {
        assert!(parse_loopback_endpoint("127.0.0.1:43123").is_ok());
        assert!(matches!(
            parse_loopback_endpoint("192.0.2.1:43123"),
            Err(Error::InvalidResponse(_))
        ));
        assert!(matches!(
            parse_loopback_endpoint("[::1]:43123"),
            Err(Error::InvalidResponse(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires AF_UNIX socket creation (EPERM in the managed sandbox)"]
    fn unix_transport_sends_and_validates_the_discovered_instance_id() {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
        };

        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("agent.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let server = std::thread::spawn(move || {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                let mut response = host_response();
                if request_index == 0 {
                    assert_eq!(request, json!({ "op": "host.status" }));
                } else {
                    assert_eq!(
                        request,
                        json!({
                            "op": "host.advisories",
                            "cursor": 0,
                            "host_instance_id": HOST_ID,
                        })
                    );
                    response["advisories"] = json!({
                        "host_instance_id": HOST_ID,
                        "schema": ADVISORY_SCHEMA,
                        "events": [],
                        "next_cursor": 0,
                        "truncated": false,
                    });
                }
                stream
                    .write_all(&serde_json::to_vec(&response).unwrap())
                    .unwrap();
            }
        });

        let client = AgentHostClient::connect(&socket_path).unwrap();
        assert_eq!(client.advisories(0).unwrap().next_cursor, 0);
        server.join().unwrap();
    }
}
