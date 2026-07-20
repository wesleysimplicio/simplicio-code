//! Fail-closed client for the independent Simplicio Agent host.
//!
//! Simplicio Code requires both Agent and Runtime, but neither product imports
//! Code. This crate owns Code's side of that boundary: it verifies the host's
//! identity, protocol versions, capabilities, and fixed advisory vocabulary
//! before exposing anything to the rest of the application. There is no
//! built-in coordinator or local fallback in this crate.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
            history_truncated: self.truncated,
        }
    }
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
fn request(_socket_path: &Path, _payload: &Value) -> Result<Value, Error> {
    Err(Error::UnsupportedTransport)
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
    fn accepts_exact_agent_host_contract() {
        let capabilities = validate_ready_host_response(&host_response(), None).unwrap();

        assert_eq!(capabilities.profile, "desktop");
        assert!(capabilities.supports("host.advisories"));
        assert_eq!(capabilities.host_instance_id(), &host_instance_id());
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
                history_truncated: false,
            }
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
