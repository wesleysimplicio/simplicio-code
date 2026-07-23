//! Transport-neutral contracts for external LLMs operating Simplicio Code.
//!
//! This crate is deliberately a protocol boundary, not an agent. It never
//! schedules work, invokes a model, executes a command, or accesses a
//! workspace. CLI, MCP, ACP, and workspace adapters pass the same [`Request`]
//! to the existing Agent/Loop/Runtime stack and return the same [`Response`].

use serde::{Deserialize, Serialize};

pub const SCHEMA_V1: &str = "simplicio.agent-native/v1";
pub const CAPABILITY_SCHEMA_V1: &str = "simplicio.agent-native-capabilities/v1";
pub const RECEIPT_SCHEMA_V1: &str = "simplicio.agent-native-receipt/v1";
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 1] = [SCHEMA_V1];
pub const DEFAULT_PAGE_SIZE: u16 = 50;
pub const MAX_PAGE_SIZE: u16 = 200;
const MAX_ID_BYTES: usize = 256;
const MAX_GOAL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Cli,
    Mcp,
    Acp,
    Workspace,
    AxiAdapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Installed,
    Compatible,
    Ready,
    Degraded,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyStatus {
    pub name: String,
    pub health: Health,
    pub version: Option<String>,
    pub reason: Option<ReasonCode>,
    /// A diagnostic command only. It must not perform an effect or enable a
    /// provider implicitly.
    pub safe_command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorReport {
    pub schema: String,
    pub health: Health,
    pub dependencies: Vec<DependencyStatus>,
    pub capabilities: CapabilityManifest,
}

impl DoctorReport {
    pub fn new(dependencies: Vec<DependencyStatus>, capabilities: CapabilityManifest) -> Self {
        let health = aggregate_health(&dependencies);
        Self {
            schema: SCHEMA_V1.into(),
            health,
            dependencies,
            capabilities,
        }
    }
}

fn aggregate_health(dependencies: &[DependencyStatus]) -> Health {
    if dependencies
        .iter()
        .any(|item| item.health == Health::Missing)
    {
        Health::Missing
    } else if dependencies
        .iter()
        .any(|item| item.health == Health::Degraded)
    {
        Health::Degraded
    } else if dependencies.iter().all(|item| item.health == Health::Ready) {
        Health::Ready
    } else if dependencies
        .iter()
        .all(|item| matches!(item.health, Health::Ready | Health::Compatible))
    {
        Health::Compatible
    } else {
        Health::Installed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityManifest {
    pub schema: String,
    pub protocol_versions: Vec<String>,
    pub surfaces: Vec<Surface>,
    pub operations: Vec<Operation>,
    pub authority: AuthorityContract,
}

impl Default for CapabilityManifest {
    fn default() -> Self {
        Self {
            schema: CAPABILITY_SCHEMA_V1.into(),
            protocol_versions: vec![SCHEMA_V1.into()],
            surfaces: vec![
                Surface::Cli,
                Surface::Mcp,
                Surface::Acp,
                Surface::Workspace,
                Surface::AxiAdapter,
            ],
            operations: Operation::ALL.to_vec(),
            authority: AuthorityContract::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityContract {
    pub cognitive_authority: String,
    pub scheduling_authority: String,
    pub workspace_effect_authority: String,
    pub internal_provider_enabled: bool,
    pub local_llm_enabled: bool,
}

impl Default for AuthorityContract {
    fn default() -> Self {
        Self {
            cognitive_authority: "external_invoker".into(),
            scheduling_authority: "simplicio_loop_hub".into(),
            workspace_effect_authority: "simplicio_runtime".into(),
            internal_provider_enabled: false,
            local_llm_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Discover,
    Doctor,
    ListProjects,
    ListSessions,
    ListAgents,
    GetState,
    SubmitGoal,
    Attach,
    FollowEvents,
    GetReceipt,
    ApprovalIntent,
    Diff,
    Test,
    Review,
    PullRequestIntent,
    DeliveryIntent,
    BrowserIntent,
}

impl Operation {
    pub const ALL: [Self; 17] = [
        Self::Discover,
        Self::Doctor,
        Self::ListProjects,
        Self::ListSessions,
        Self::ListAgents,
        Self::GetState,
        Self::SubmitGoal,
        Self::Attach,
        Self::FollowEvents,
        Self::GetReceipt,
        Self::ApprovalIntent,
        Self::Diff,
        Self::Test,
        Self::Review,
        Self::PullRequestIntent,
        Self::DeliveryIntent,
        Self::BrowserIntent,
    ];

    pub fn is_external_effect(self) -> bool {
        matches!(
            self,
            Self::PullRequestIntent | Self::DeliveryIntent | Self::BrowserIntent
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema: String,
    pub request_id: String,
    pub surface: Surface,
    pub operation: Operation,
    pub authority: ExternalAuthority,
    pub target: Option<Target>,
    pub page: Option<PageRequest>,
    pub goal: Option<String>,
    pub intent: Option<GovernedIntent>,
}

impl Request {
    pub fn from_json(input: &str) -> Result<Self, ProtocolError> {
        let request: Self = serde_json::from_str(input)
            .map_err(|_| ProtocolError::malformed("request payload is malformed"))?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema != SCHEMA_V1 {
            return Err(ProtocolError::malformed("unsupported schema"));
        }
        validate_id("request_id", &self.request_id)?;
        if self.authority.kind != "external_llm" {
            return Err(ProtocolError::malformed("authority must be external_llm"));
        }
        validate_id("invoker_id", &self.authority.invoker_id)?;
        if let Some(page) = &self.page {
            page.validate()?;
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }
        if let Some(goal) = &self.goal
            && (goal.is_empty() || goal.len() > MAX_GOAL_BYTES)
        {
            return Err(ProtocolError::malformed("goal size is invalid"));
        }
        if self.operation == Operation::SubmitGoal && self.goal.is_none() {
            return Err(ProtocolError::malformed("submit_goal requires goal"));
        }
        if self.operation.is_external_effect() {
            let intent = self.intent.as_ref().ok_or_else(|| {
                ProtocolError::approval("external effect requires a governed intent")
            })?;
            intent.validate()?;
        } else if self.intent.is_some() {
            return Err(ProtocolError::malformed(
                "intent is only valid for an external effect",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAuthority {
    pub kind: String,
    pub invoker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
}

impl Target {
    fn validate(&self) -> Result<(), ProtocolError> {
        for (field, value) in [
            ("project_id", self.project_id.as_deref()),
            ("session_id", self.session_id.as_deref()),
            ("agent_id", self.agent_id.as_deref()),
        ] {
            if let Some(value) = value {
                validate_id(field, value)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    pub limit: u16,
    pub cursor: Option<String>,
}

impl PageRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.limit == 0 || self.limit > MAX_PAGE_SIZE {
            return Err(ProtocolError::malformed("page limit is outside 1..=200"));
        }
        if let Some(cursor) = &self.cursor {
            validate_id("cursor", cursor)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernedIntent {
    pub intent_id: String,
    pub approval_receipt_id: String,
    pub policy_revision: u64,
    pub expected_remote_revision: String,
}

impl GovernedIntent {
    fn validate(&self) -> Result<(), ProtocolError> {
        validate_id("intent_id", &self.intent_id)?;
        validate_id("approval_receipt_id", &self.approval_receipt_id)?;
        validate_id("expected_remote_revision", &self.expected_remote_revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub schema: String,
    pub request_id: String,
    pub result: Option<serde_json::Value>,
    pub page: Option<PageInfo>,
    pub receipt: Option<ReceiptRef>,
    pub error: Option<ErrorBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInfo {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRef {
    pub schema: String,
    pub receipt_id: String,
    pub effect: EffectState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    NotStarted,
    Denied,
    Completed,
    EffectUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    DependencyMissing,
    Quota,
    ApprovalRequired,
    StaleCursor,
    Blocked,
    EffectUnknown,
    MalformedPayload,
    IncompatibleVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub reason: ReasonCode,
    pub message: String,
    pub retryable: bool,
    pub safe_command: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{body:?}")]
pub struct ProtocolError {
    pub body: ErrorBody,
}

impl ProtocolError {
    fn malformed(message: &str) -> Self {
        Self {
            body: ErrorBody {
                reason: ReasonCode::MalformedPayload,
                message: message.into(),
                retryable: false,
                safe_command: None,
            },
        }
    }
    fn approval(message: &str) -> Self {
        Self {
            body: ErrorBody {
                reason: ReasonCode::ApprovalRequired,
                message: message.into(),
                retryable: false,
                safe_command: None,
            },
        }
    }
}

fn validate_id(field: &str, value: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ProtocolError::malformed(&format!("{field} is invalid")));
    }
    Ok(())
}

fn sensitive_key(key: &str) -> bool {
    let normalized: String = key
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect();
    matches!(
        normalized.as_str(),
        "token"
            | "accesstoken"
            | "refreshtoken"
            | "secret"
            | "password"
            | "apikey"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "prompt"
            | "content"
            | "code"
            | "sourcecode"
    )
}

/// Removes known secret/content fields before a value reaches logs or a
/// receipt. Redaction is recursive and key based so adapters share behavior.
pub fn redact(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if sensitive_key(key) {
                    *value = serde_json::Value::String("[REDACTED]".into());
                } else {
                    redact(value);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn request(operation: Operation) -> Request {
        Request {
            schema: SCHEMA_V1.into(),
            request_id: "req-1".into(),
            surface: Surface::Mcp,
            operation,
            authority: ExternalAuthority {
                kind: "external_llm".into(),
                invoker_id: "caller-1".into(),
            },
            target: None,
            page: None,
            goal: None,
            intent: None,
        }
    }

    #[test]
    fn manifest_cannot_enable_an_internal_model() {
        let manifest = CapabilityManifest::default();
        assert_eq!(manifest.authority.cognitive_authority, "external_invoker");
        assert!(!manifest.authority.internal_provider_enabled);
        assert!(!manifest.authority.local_llm_enabled);
    }

    #[test]
    fn external_effect_requires_approval_and_requery_revision() {
        let mut value = request(Operation::PullRequestIntent);
        assert_eq!(
            value.validate().unwrap_err().body.reason,
            ReasonCode::ApprovalRequired
        );
        value.intent = Some(GovernedIntent {
            intent_id: "intent-1".into(),
            approval_receipt_id: "approval-1".into(),
            policy_revision: 7,
            expected_remote_revision: "sha-1".into(),
        });
        assert!(value.validate().is_ok());
    }

    #[test]
    fn malformed_json_and_unknown_fields_fail_closed() {
        assert_eq!(
            Request::from_json("not-json").unwrap_err().body.reason,
            ReasonCode::MalformedPayload
        );
        let payload = r#"{
            "schema":"simplicio.agent-native/v1",
            "request_id":"req-1",
            "surface":"mcp",
            "operation":"get_state",
            "authority":{"kind":"external_llm","invoker_id":"caller-1"},
            "unexpected":"reject"
        }"#;
        assert_eq!(
            Request::from_json(payload).unwrap_err().body.reason,
            ReasonCode::MalformedPayload
        );
    }

    #[test]
    fn all_surfaces_serialize_the_same_operation_semantics() {
        for surface in [
            Surface::Cli,
            Surface::Mcp,
            Surface::Acp,
            Surface::Workspace,
            Surface::AxiAdapter,
        ] {
            let mut value = request(Operation::GetState);
            value.surface = surface;
            assert!(value.validate().is_ok());
            assert_eq!(
                serde_json::to_value(value).unwrap()["operation"],
                "get_state"
            );
        }
    }

    #[test]
    fn doctor_distinguishes_degraded_and_missing() {
        let dependency = |health| DependencyStatus {
            name: "runtime".into(),
            health,
            version: None,
            reason: None,
            safe_command: None,
        };
        assert_eq!(
            DoctorReport::new(
                vec![dependency(Health::Degraded)],
                CapabilityManifest::default()
            )
            .health,
            Health::Degraded
        );
        assert_eq!(
            DoctorReport::new(
                vec![dependency(Health::Missing)],
                CapabilityManifest::default()
            )
            .health,
            Health::Missing
        );
    }

    #[test]
    fn nested_secrets_and_source_content_are_redacted() {
        let mut value = serde_json::json!({"token":"secret", "nested":[{"code":"private", "api-key":"key"}], "receipt_id":"safe"});
        redact(&mut value);
        assert_eq!(
            value,
            serde_json::json!({"token":"[REDACTED]", "nested":[{"code":"[REDACTED]", "api-key":"[REDACTED]"}], "receipt_id":"safe"})
        );
    }

    proptest! {
        #[test]
        fn arbitrary_page_limits_fail_closed(limit in any::<u16>()) {
            let result = PageRequest { limit, cursor: None }.validate();
            prop_assert_eq!(result.is_ok(), (1..=MAX_PAGE_SIZE).contains(&limit));
        }

        #[test]
        fn redaction_never_retains_token(token in ".{1,128}") {
            let mut value = serde_json::json!({"access_token": token});
            redact(&mut value);
            prop_assert_eq!(&value["access_token"], "[REDACTED]");
        }
    }
}
