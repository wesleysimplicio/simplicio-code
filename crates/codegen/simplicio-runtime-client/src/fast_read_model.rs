//! Safe read-only projection for Code surfaces consuming Fast status.
use super::fast_surface::{FastSurfaceAction, FastSurfaceStatus};
use serde::{Deserialize, Serialize};

pub const FAST_READ_MODEL_SCHEMA_V1: &str = "simplicio.code.fast-read-model/v1";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FastSurfaceReadModel {
    pub schema: String,
    pub mode: String,
    pub requested_engine: String,
    pub selected_engine: String,
    pub runtime_schema: String,
    pub conformance: String,
    pub generation: String,
    pub snapshot_sha256: Option<String>,
    pub span_count: usize,
    pub complete: bool,
    pub stale: bool,
    pub fallback_reason: Option<String>,
    pub local_llm_started: bool,
    pub apply_allowed: bool,
    pub apply_block_reason: Option<String>,
}

impl FastSurfaceReadModel {
    pub fn from_status(status: &FastSurfaceStatus) -> Self {
        let apply_block_reason = status
            .authorize(FastSurfaceAction::Apply)
            .err()
            .map(|error| error.to_string());
        Self {
            schema: FAST_READ_MODEL_SCHEMA_V1.into(),
            mode: status.mode.clone(),
            requested_engine: status.requested_engine.clone(),
            selected_engine: status.selected_engine.clone(),
            runtime_schema: status.runtime_schema.clone(),
            conformance: status.conformance.clone(),
            generation: status.generation.clone(),
            snapshot_sha256: status.snapshot_sha256.clone(),
            span_count: status.span_count,
            complete: status.complete,
            stale: status.stale,
            fallback_reason: status.fallback_reason.clone(),
            local_llm_started: status.local_llm_started,
            apply_allowed: apply_block_reason.is_none(),
            apply_block_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> FastSurfaceStatus {
        FastSurfaceStatus {
            schema: "simplicio.code.fast-surface/v1".into(),
            mode: "full".into(),
            requested_engine: "auto".into(),
            selected_engine: "rust".into(),
            runtime_schema: "simplicio.context-packet/v1".into(),
            conformance: "pass".into(),
            generation: "SFAST001:fresh".into(),
            snapshot_sha256: Some("a".repeat(64)),
            span_count: 2,
            complete: true,
            stale: false,
            fallback_reason: None,
            local_llm_started: false,
        }
    }

    #[test]
    fn fresh_status_projects_safe_read_model_and_allows_apply() {
        let model = FastSurfaceReadModel::from_status(&status());

        assert_eq!(model.schema, FAST_READ_MODEL_SCHEMA_V1);
        assert_eq!(model.selected_engine, "rust");
        assert!(model.apply_allowed);
        assert_eq!(model.apply_block_reason, None);
        assert!(!model.local_llm_started);
    }

    #[test]
    fn stale_status_projects_explicit_block_without_green_state() {
        let mut status = status();
        status.stale = true;
        status.fallback_reason = Some("snapshot-mismatch".into());

        let model = FastSurfaceReadModel::from_status(&status);

        assert!(!model.apply_allowed);
        assert!(model.stale);
        assert_eq!(model.fallback_reason.as_deref(), Some("snapshot-mismatch"));
        assert!(model.apply_block_reason.unwrap().contains("stale"));
    }

    #[test]
    fn local_llm_status_projects_provenance_and_blocks_apply() {
        let mut status = status();
        status.local_llm_started = true;

        let model = FastSurfaceReadModel::from_status(&status);

        assert!(model.local_llm_started);
        assert!(!model.apply_allowed);
        assert!(model.apply_block_reason.unwrap().contains("local LLM"));
    }
}
