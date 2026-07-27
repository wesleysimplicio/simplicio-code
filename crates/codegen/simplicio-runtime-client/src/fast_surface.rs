//! Receipt-backed Fast state for Code Full and Loop standalone surfaces.
use super::FastContextPacket;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FAST_SURFACE_SCHEMA_V1: &str = "simplicio.code.fast-surface/v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FastSurfaceMode {
    Full,
    LoopStandalone,
    Off,
}

impl FastSurfaceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::LoopStandalone => "loop-standalone",
            Self::Off => "off",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "loop" | "loop-standalone" | "standalone" => Some(Self::LoopStandalone),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FastSurfaceAction {
    Diagnose,
    Refresh,
    RollbackPython,
    Disable,
    Apply,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FastSnapshotIdentity {
    pub generation: String,
    #[serde(default)]
    pub snapshot_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FastSurfaceStatus {
    pub schema: String,
    pub mode: String,
    pub requested_engine: String,
    pub selected_engine: String,
    pub runtime_schema: String,
    pub conformance: String,
    pub generation: String,
    #[serde(default)]
    pub snapshot_sha256: Option<String>,
    pub span_count: usize,
    pub complete: bool,
    pub stale: bool,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    pub local_llm_started: bool,
}

impl FastSurfaceStatus {
    pub fn from_packet(
        packet: &FastContextPacket,
        mode: FastSurfaceMode,
        requested_engine: impl Into<String>,
        expected: Option<&FastSnapshotIdentity>,
        fallback_reason: Option<String>,
    ) -> Self {
        let snapshot_sha256 = packet.provenance.snapshot_sha256.clone();
        let stale = expected.is_some_and(|identity| {
            identity.generation != packet.generation
                || identity
                    .snapshot_sha256
                    .as_ref()
                    .is_some_and(|digest| Some(digest) != snapshot_sha256.as_ref())
        });
        Self {
            schema: FAST_SURFACE_SCHEMA_V1.into(),
            mode: mode.as_str().into(),
            requested_engine: requested_engine.into(),
            selected_engine: packet.provenance.engine.clone(),
            runtime_schema: packet.schema.clone(),
            conformance: if packet.complete && packet.fidelity == "exact" {
                "pass"
            } else {
                "degraded"
            }
            .into(),
            generation: packet.generation.clone(),
            snapshot_sha256,
            span_count: packet.spans.len(),
            complete: packet.complete,
            stale,
            fallback_reason,
            local_llm_started: packet.provenance.local_llm_started,
        }
    }

    pub fn authorize(&self, action: FastSurfaceAction) -> Result<(), FastSurfaceError> {
        if action != FastSurfaceAction::Apply {
            return Ok(());
        }
        if self.stale {
            return Err(FastSurfaceError::StaleGeneration);
        }
        if self.local_llm_started {
            return Err(FastSurfaceError::LocalLlmForbidden);
        }
        if self.mode == FastSurfaceMode::Off.as_str() {
            return Err(FastSurfaceError::FastDisabled);
        }
        if !self.complete {
            return Err(FastSurfaceError::IncompleteContext);
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FastSurfaceError {
    #[error("productive Fast action refused because generation or snapshot digest is stale")]
    StaleGeneration,
    #[error("productive Fast action refused because local LLM provenance is forbidden")]
    LocalLlmForbidden,
    #[error("productive Fast action refused while Fast is off")]
    FastDisabled,
    #[error("productive Fast action requires complete context")]
    IncompleteContext,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn packet(name: &str, complete: bool, local_llm_started: bool) -> FastContextPacket {
        FastContextPacket {
            schema: super::super::FAST_CONTEXT_SCHEMA_V1.into(),
            source: "runtime-fast".into(),
            generation: format!("SFAST001:{name}"),
            fidelity: "exact".into(),
            complete,
            provenance: super::super::FastContextProvenance {
                provider: "simplicio-fast".into(),
                engine: "rust-fast".into(),
                local_llm_started,
                snapshot_generation: Some(format!("SFAST001:{name}")),
                snapshot_sha256: Some(format!("{name:0<64}")),
            },
            spans: vec![json!({"file":"src/lib.rs","content":"safe"})],
            content_sha256: "a".repeat(64),
            fast_receipt: json!({"schema":"simplicio.fast.receipt/v1"}),
        }
    }

    #[test]
    fn full_and_loop_standalone_are_distinct_surface_modes() {
        let full = FastSurfaceStatus::from_packet(
            &packet("full", true, false),
            FastSurfaceMode::Full,
            "auto",
            None,
            None,
        );
        let loop_status = FastSurfaceStatus::from_packet(
            &packet("loop", true, false),
            FastSurfaceMode::LoopStandalone,
            "rust",
            None,
            None,
        );
        assert_eq!(full.mode, "full");
        assert_eq!(loop_status.mode, "loop-standalone");
        assert_ne!(full.mode, loop_status.mode);
        assert_eq!(full.conformance, "pass");
        assert!(!full.local_llm_started);
    }

    #[test]
    fn stale_generation_blocks_apply() {
        let status = FastSurfaceStatus::from_packet(
            &packet("fresh", true, false),
            FastSurfaceMode::Full,
            "auto",
            Some(&FastSnapshotIdentity {
                generation: "SFAST001:other".into(),
                snapshot_sha256: None,
            }),
            None,
        );
        assert!(status.stale);
        assert_eq!(
            status.authorize(FastSurfaceAction::Apply),
            Err(FastSurfaceError::StaleGeneration)
        );
    }

    #[test]
    fn fresh_rust_packet_allows_apply_without_local_llm() {
        let status = FastSurfaceStatus::from_packet(
            &packet("fresh", true, false),
            FastSurfaceMode::LoopStandalone,
            "rust",
            Some(&FastSnapshotIdentity {
                generation: "SFAST001:fresh".into(),
                snapshot_sha256: Some(format!("{:0<64}", "fresh")),
            }),
            None,
        );
        assert!(!status.stale);
        assert_eq!(status.authorize(FastSurfaceAction::Apply), Ok(()));
    }

    #[test]
    fn off_mode_is_diagnosable_but_cannot_apply() {
        let status = FastSurfaceStatus::from_packet(
            &packet("off", true, false),
            FastSurfaceMode::Off,
            "off",
            None,
            Some("operator-disabled".into()),
        );
        assert_eq!(status.authorize(FastSurfaceAction::Diagnose), Ok(()));
        assert_eq!(status.authorize(FastSurfaceAction::Disable), Ok(()));
        assert_eq!(
            status.authorize(FastSurfaceAction::Apply),
            Err(FastSurfaceError::FastDisabled)
        );
        assert_eq!(status.fallback_reason.as_deref(), Some("operator-disabled"));
    }

    #[test]
    fn local_llm_provenance_is_rejected_for_apply() {
        let status = FastSurfaceStatus::from_packet(
            &packet("local", true, true),
            FastSurfaceMode::Full,
            "auto",
            None,
            None,
        );
        assert_eq!(
            status.authorize(FastSurfaceAction::Apply),
            Err(FastSurfaceError::LocalLlmForbidden)
        );
    }
}
