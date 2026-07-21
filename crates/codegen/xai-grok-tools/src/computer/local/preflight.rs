//! Installed AgentHost + Runtime preflight for productive Code mode.
//!
//! This module is deliberately diagnostic-only.  It performs the independent
//! protocol handshakes, validates the causal identity, and returns a stable
//! report.  It never starts a turn, calls a Runtime tool, writes a file, or
//! provides a local/builtin fallback.

use std::path::Path;

use serde::{Deserialize, Serialize};
use simplicio_agent_client::{AgentHostCoordinator, CausalIdentity, Error as AgentError};
use simplicio_runtime_client::{Error as RuntimeError, RuntimeClient};

pub const PREFLIGHT_SCHEMA: &str = "simplicio.code-agent-runtime-preflight/v1";
pub const PREFLIGHT_PROTOCOL_VERSION: u64 = 1;
pub const OFFLINE_FIXTURE_MODE: &str = "offline_protocol_fixture";
pub const INSTALLED_MODE: &str = "installed";

const REQUIRED_RUNTIME_TOOLS: [&str; 8] = [
    "simplicio_edit",
    "simplicio_exec",
    "simplicio_file_read",
    "simplicio_fs_delete",
    "simplicio_fs_list",
    "simplicio_fs_stat",
    "simplicio_fs_write",
    "simplicio_search",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ready,
    Missing,
    Incompatible,
    ProtocolOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckDiagnostic {
    pub component: String,
    pub status: CheckStatus,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalIdentityDiagnostic {
    pub status: CheckStatus,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductivePreflightReport {
    pub schema: String,
    pub protocol_version: u64,
    pub mode: String,
    pub effects_enabled: bool,
    pub agent_host: CheckDiagnostic,
    pub runtime: CheckDiagnostic,
    pub causal_identity: CausalIdentityDiagnostic,
}

impl ProductivePreflightReport {
    pub fn can_enter_productive_mode(&self) -> bool {
        self.mode == INSTALLED_MODE
            && !self.effects_enabled
            && self.agent_host.status == CheckStatus::Ready
            && self.runtime.status == CheckStatus::Ready
            && self.causal_identity.status == CheckStatus::Ready
    }

    pub fn is_protocol_valid(&self) -> bool {
        self.schema == PREFLIGHT_SCHEMA
            && self.protocol_version == PREFLIGHT_PROTOCOL_VERSION
            && self.causal_identity.status == CheckStatus::Ready
    }
}

/// Runs the real installed dependency checks.  The returned diagnostics use
/// stable codes and sorted capability names so CI and support tooling can
/// compare reports without depending on socket paths or process IDs.
pub fn run_installed_preflight(
    workspace: &Path,
    agent_socket: &Path,
    profile: &str,
    identity: &CausalIdentity,
) -> ProductivePreflightReport {
    let causal_identity = validate_identity(identity);
    let agent_host = match AgentHostCoordinator::connect_at(profile, agent_socket) {
        Ok(_) => ready("agent_host.ready", "installed AgentHost handshake accepted"),
        Err(error) => agent_error(&error),
    };
    let runtime = match RuntimeClient::spawn_in(workspace) {
        Ok(client) => runtime_capability_check(client.capabilities()),
        Err(error) => runtime_error(&error),
    };

    ProductivePreflightReport {
        schema: PREFLIGHT_SCHEMA.into(),
        protocol_version: PREFLIGHT_PROTOCOL_VERSION,
        mode: INSTALLED_MODE.into(),
        // A preflight never grants effect authority.  The productive gate
        // remains `SimplicioRuntimeFs::with_runtime` + AgentHost turn policy.
        effects_enabled: false,
        agent_host,
        runtime,
        causal_identity,
    }
}

/// Offline fixture used by protocol/unit tests.  It intentionally cannot
/// report `Ready` or enable effects: it proves only serialization, schema,
/// capability vocabulary, and causal identity validation.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfflineContractFixture;

impl OfflineContractFixture {
    pub fn validate(identity: &CausalIdentity) -> ProductivePreflightReport {
        let causal_identity = validate_identity(identity);
        let detail = "offline fixture validates protocol only; installed E2E is not proven";
        ProductivePreflightReport {
            schema: PREFLIGHT_SCHEMA.into(),
            protocol_version: PREFLIGHT_PROTOCOL_VERSION,
            mode: OFFLINE_FIXTURE_MODE.into(),
            effects_enabled: false,
            agent_host: CheckDiagnostic {
                component: "agent_host".into(),
                status: CheckStatus::ProtocolOnly,
                code: "agent_host.fixture_protocol_only".into(),
                detail: detail.into(),
            },
            runtime: CheckDiagnostic {
                component: "runtime".into(),
                status: CheckStatus::ProtocolOnly,
                code: "runtime.fixture_protocol_only".into(),
                detail: detail.into(),
            },
            causal_identity,
        }
    }
}

fn validate_identity(identity: &CausalIdentity) -> CausalIdentityDiagnostic {
    match identity.validate() {
        Ok(()) => CausalIdentityDiagnostic {
            status: CheckStatus::Ready,
            code: "causal_identity.valid".into(),
            detail: "causal identity is complete and internally consistent".into(),
        },
        Err(error) => CausalIdentityDiagnostic {
            status: CheckStatus::Incompatible,
            code: "causal_identity.invalid".into(),
            detail: error.to_string(),
        },
    }
}

fn ready(code: &str, detail: &str) -> CheckDiagnostic {
    CheckDiagnostic {
        component: code.split('.').next().unwrap_or(code).into(),
        status: CheckStatus::Ready,
        code: code.into(),
        detail: detail.into(),
    }
}

fn agent_error(error: &AgentError) -> CheckDiagnostic {
    let (status, code, detail) = match error {
        AgentError::AgentNotFound(_) => (
            CheckStatus::Missing,
            "agent_host.missing",
            "installed AgentHost socket was not found",
        ),
        AgentError::UnsupportedTransport => (
            CheckStatus::Incompatible,
            "agent_host.transport_unsupported",
            "installed AgentHost transport is unsupported",
        ),
        AgentError::ProtocolMismatch(_) => (
            CheckStatus::Incompatible,
            "agent_host.protocol_mismatch",
            "installed AgentHost protocol is incompatible",
        ),
        AgentError::CapabilityMismatch { .. } => (
            CheckStatus::Incompatible,
            "agent_host.capabilities_missing",
            "installed AgentHost lacks a required Code capability",
        ),
        AgentError::OperationRejected => (
            CheckStatus::Incompatible,
            "agent_host.not_ready",
            "installed AgentHost is not ready for productive Code mode",
        ),
        AgentError::InvalidHostInstanceId | AgentError::HostInstanceMismatch => (
            CheckStatus::Incompatible,
            "agent_host.identity_invalid",
            "installed AgentHost instance identity is invalid",
        ),
        _ => (
            CheckStatus::Incompatible,
            "agent_host.incompatible",
            "installed AgentHost failed the Code contract",
        ),
    };
    CheckDiagnostic {
        component: "agent_host".into(),
        status,
        code: code.into(),
        detail: detail.into(),
    }
}

fn runtime_error(error: &RuntimeError) -> CheckDiagnostic {
    let (status, code, detail) = match error {
        RuntimeError::RuntimeNotFound => (
            CheckStatus::Missing,
            "runtime.missing",
            "installed Simplicio Runtime executable was not found",
        ),
        RuntimeError::IdentityMismatch(_) => (
            CheckStatus::Incompatible,
            "runtime.identity_mismatch",
            "installed Runtime announced an incompatible server identity",
        ),
        RuntimeError::CapabilityMismatch { .. } => (
            CheckStatus::Incompatible,
            "runtime.capabilities_missing",
            "installed Runtime lacks a required Code capability",
        ),
        RuntimeError::CompatibilityMismatch(_) => (
            CheckStatus::Incompatible,
            "runtime.release_incompatible",
            "installed Runtime release is incompatible",
        ),
        RuntimeError::HandshakeTimeout { .. } => (
            CheckStatus::Incompatible,
            "runtime.handshake_timeout",
            "installed Runtime handshake timed out",
        ),
        _ => (
            CheckStatus::Incompatible,
            "runtime.incompatible",
            "installed Runtime failed the Code contract",
        ),
    };
    CheckDiagnostic {
        component: "runtime".into(),
        status,
        code: code.into(),
        detail: detail.into(),
    }
}

fn runtime_capability_check(capabilities: &simplicio_runtime_client::RuntimeCapabilities) -> CheckDiagnostic {
    let missing = REQUIRED_RUNTIME_TOOLS
        .iter()
        .filter(|tool| !capabilities.supports(tool))
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return ready("runtime.ready", "installed Runtime handshake and Code tool contract accepted");
    }
    CheckDiagnostic {
        component: "runtime".into(),
        status: CheckStatus::Incompatible,
        code: "runtime.capabilities_missing".into(),
        detail: format!("missing required tools: {}", missing.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> CausalIdentity {
        CausalIdentity::new("workspace", "session", "turn-1").unwrap()
    }

    #[test]
    fn offline_fixture_is_protocol_only_and_never_ready_for_effects() {
        let report = OfflineContractFixture::validate(&identity());
        assert!(report.is_protocol_valid());
        assert!(!report.can_enter_productive_mode());
        assert!(!report.effects_enabled);
        assert_eq!(report.mode, OFFLINE_FIXTURE_MODE);
        assert_eq!(report.agent_host.status, CheckStatus::ProtocolOnly);
        assert_eq!(report.runtime.status, CheckStatus::ProtocolOnly);
    }

    #[test]
    fn invalid_causal_identity_is_deterministic_and_blocks_fixture() {
        let mut identity = identity();
        identity.turn_id.clear();
        let report = OfflineContractFixture::validate(&identity);
        assert_eq!(report.causal_identity.status, CheckStatus::Incompatible);
        assert_eq!(report.causal_identity.code, "causal_identity.invalid");
        assert!(!report.can_enter_productive_mode());
    }

    #[test]
    fn required_runtime_tools_are_checked_in_sorted_contract_order() {
        let capabilities = simplicio_runtime_client::RuntimeCapabilities {
            schema: "simplicio.code-mcp/v1".into(),
            protocol_version: "2024-11-05".into(),
            server_name: "simplicio".into(),
            server_version: None,
            component_release: None,
            tools: ["simplicio_exec", "simplicio_fs_list"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        };
        let diagnostic = runtime_capability_check(&capabilities);
        assert_eq!(diagnostic.status, CheckStatus::Incompatible);
        assert!(diagnostic.detail.contains("simplicio_edit"));
        assert!(diagnostic.detail.contains("simplicio_search"));
    }

    #[test]
    fn missing_installed_dependencies_have_stable_diagnostics() {
        let missing_binary = std::env::temp_dir().join("simplicio-runtime-preflight-missing");
        let missing_socket = std::env::temp_dir().join("simplicio-agent-preflight-missing");
        unsafe {
            std::env::set_var("SIMPLICIO_BIN", &missing_binary);
        }
        let report = run_installed_preflight(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &missing_socket,
            "desktop",
            &identity(),
        );
        unsafe {
            std::env::remove_var("SIMPLICIO_BIN");
        }
        assert_eq!(report.agent_host.status, CheckStatus::Missing);
        assert_eq!(report.agent_host.code, "agent_host.missing");
        assert_eq!(report.runtime.status, CheckStatus::Missing);
        assert_eq!(report.runtime.code, "runtime.missing");
        assert!(!report.can_enter_productive_mode());
    }
}
