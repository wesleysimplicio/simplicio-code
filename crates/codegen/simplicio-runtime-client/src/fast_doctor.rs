//! CLI/doctor-facing Fast surface summary for Full and Loop standalone modes.
//!
//! Never reads `.sfast` directly; projects only receipt-backed status.

use crate::fast_read_model::FastSurfaceReadModel;
use crate::fast_surface::{FastSurfaceAction, FastSurfaceMode, FastSurfaceStatus};
use serde::{Deserialize, Serialize};

pub const FAST_DOCTOR_SCHEMA_V1: &str = "simplicio.code.fast-doctor/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastDoctorReport {
    pub schema: String,
    pub mode: String,
    pub requested_engine: String,
    pub selected_engine: String,
    pub generation: String,
    pub conformance: String,
    pub stale: bool,
    pub apply_allowed: bool,
    pub apply_block_reason: Option<String>,
    pub fallback_reason: Option<String>,
    pub local_llm_started: bool,
    pub diagnostic_actions: Vec<String>,
    pub reads_sfast_directly: bool,
}

impl FastDoctorReport {
    pub fn from_status(status: &FastSurfaceStatus) -> Self {
        let model = FastSurfaceReadModel::from_status(status);
        let mut diagnostic_actions = vec![
            FastSurfaceAction::Diagnose.as_action_name().to_string(),
            FastSurfaceAction::Refresh.as_action_name().to_string(),
        ];
        if status.selected_engine.to_ascii_lowercase().contains("python")
            || status.fallback_reason.is_some()
        {
            diagnostic_actions.push(FastSurfaceAction::RollbackPython.as_action_name().to_string());
        }
        diagnostic_actions.push(FastSurfaceAction::Disable.as_action_name().to_string());
        Self {
            schema: FAST_DOCTOR_SCHEMA_V1.into(),
            mode: model.mode,
            requested_engine: model.requested_engine,
            selected_engine: model.selected_engine,
            generation: model.generation,
            conformance: model.conformance,
            stale: model.stale,
            apply_allowed: model.apply_allowed,
            apply_block_reason: model.apply_block_reason,
            fallback_reason: model.fallback_reason,
            local_llm_started: model.local_llm_started,
            diagnostic_actions,
            reads_sfast_directly: false,
        }
    }

    pub fn modes_are_distinguishable(full: &Self, standalone: &Self) -> bool {
        full.mode == FastSurfaceMode::Full.as_str()
            && standalone.mode == FastSurfaceMode::LoopStandalone.as_str()
            && full.mode != standalone.mode
    }
}

trait ActionName {
    fn as_action_name(self) -> &'static str;
}

impl ActionName for FastSurfaceAction {
    fn as_action_name(self) -> &'static str {
        match self {
            Self::Diagnose => "diagnose",
            Self::Refresh => "refresh",
            Self::RollbackPython => "rollback-python",
            Self::Disable => "disable",
            Self::Apply => "apply",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_surface::{FastSnapshotIdentity, FastSurfaceMode};

    fn status(mode: FastSurfaceMode, engine: &str, stale: bool) -> FastSurfaceStatus {
        FastSurfaceStatus {
            schema: "simplicio.code.fast-surface/v1".into(),
            mode: mode.as_str().into(),
            requested_engine: "auto".into(),
            selected_engine: engine.into(),
            runtime_schema: "simplicio.context-packet/v1".into(),
            conformance: "pass".into(),
            generation: "SFAST001:demo".into(),
            snapshot_sha256: Some("b".repeat(64)),
            span_count: 1,
            complete: true,
            stale,
            fallback_reason: if engine == "python" {
                Some("rust_unavailable".into())
            } else {
                None
            },
            local_llm_started: false,
        }
    }

    #[test]
    fn full_and_loop_standalone_reports_are_distinct() {
        let full = FastDoctorReport::from_status(&status(FastSurfaceMode::Full, "rust", false));
        let r#loop =
            FastDoctorReport::from_status(&status(FastSurfaceMode::LoopStandalone, "rust", false));
        assert!(FastDoctorReport::modes_are_distinguishable(&full, &r#loop));
        assert!(!full.reads_sfast_directly);
        assert!(full.apply_allowed);
    }

    #[test]
    fn stale_blocks_apply_in_doctor_report() {
        let report = FastDoctorReport::from_status(&status(FastSurfaceMode::Full, "rust", true));
        assert!(report.stale);
        assert!(!report.apply_allowed);
        assert!(report.apply_block_reason.is_some());
    }

    #[test]
    fn python_fallback_exposes_reason_and_rollback_action() {
        let report =
            FastDoctorReport::from_status(&status(FastSurfaceMode::Full, "python", false));
        assert_eq!(report.fallback_reason.as_deref(), Some("rust_unavailable"));
        assert!(report
            .diagnostic_actions
            .iter()
            .any(|a| a == "rollback-python"));
    }

    #[test]
    fn identity_helper_is_available_for_guards() {
        let identity = FastSnapshotIdentity {
            generation: "SFAST001:demo".into(),
            snapshot_sha256: Some("b".repeat(64)),
        };
        assert_eq!(identity.generation, "SFAST001:demo");
    }
}
