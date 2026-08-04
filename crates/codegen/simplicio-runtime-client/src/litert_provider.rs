//! LiteRT / local-provider surface for Code — Runtime-mediated only.
//!
//! Code never links LiteRT, never opens model paths, and never runs tools outside
//! the existing Runtime pipeline. Discovery, stream, cancel, and receipts are
//! projected from Runtime provider envelopes.

use serde::{Deserialize, Serialize};

pub const LITERT_PROVIDER_SCHEMA_V1: &str = "simplicio.code.litert-provider/v1";
pub const FEATURE_FLAG_ENV: &str = "SIMPLICIO_CODE_LITERT";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferencePath {
    RuntimeLocal,
    RuntimeRemote,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapability {
    pub id: String,
    pub device: String,
    pub multimodal: bool,
    pub max_context_tokens: u32,
    pub health: ProviderHealth,
    #[serde(default)]
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSelection {
    pub schema: String,
    pub feature_enabled: bool,
    pub requested_provider: String,
    pub effective_provider: String,
    pub path: InferencePath,
    pub device: String,
    pub health: ProviderHealth,
    #[serde(default)]
    pub fallback_reason: Option<String>,
    pub remote_fallback_requires_consent: bool,
    pub remote_fallback_consented: bool,
    pub queue_depth: u32,
    pub cancel_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub request_id: String,
    pub session_id: String,
    pub provider_id: String,
    pub prompt_tokens: u32,
    pub context_limit: u32,
    #[serde(default)]
    pub attachment_handles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferenceReceipt {
    pub schema: String,
    pub request_id: String,
    pub attempted_provider: String,
    pub effective_provider: String,
    pub path: InferencePath,
    pub cancelled: bool,
    pub status: String,
    #[serde(default)]
    pub reason_code: Option<String>,
    /// Prompt/secret material must never appear here.
    pub redacted: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LiteRtError {
    #[error("LiteRT feature flag disabled")]
    FeatureDisabled,
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    #[error("context limit exceeded ({used} > {limit})")]
    ContextLimit { used: u32, limit: u32 },
    #[error("remote fallback requires explicit consent")]
    ConsentRequired,
    #[error("attachment handle rejected: {0}")]
    AttachmentRejected(String),
    #[error("model path access is forbidden in Code")]
    ModelPathForbidden,
    #[error("direct LiteRT backend access is forbidden")]
    DirectBackendForbidden,
}

pub fn feature_enabled_from_env(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on" | "enabled")
    )
}

pub fn feature_enabled() -> bool {
    feature_enabled_from_env(std::env::var(FEATURE_FLAG_ENV).ok().as_deref())
}

/// Reject any client-side model path or direct backend handle.
pub fn forbid_direct_backend(path_or_uri: &str) -> Result<(), LiteRtError> {
    let lower = path_or_uri.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return Err(LiteRtError::ModelPathForbidden);
    }
    if lower.contains("..")
        || lower.starts_with('/')
        || lower.contains(":\\")
        || lower.starts_with("file:")
        || lower.contains("litert://local")
        || lower.ends_with(".tflite")
        || lower.ends_with(".gguf")
        || lower.ends_with(".bin")
    {
        return Err(LiteRtError::ModelPathForbidden);
    }
    if lower.starts_with("litert-native:") || lower.starts_with("direct:") {
        return Err(LiteRtError::DirectBackendForbidden);
    }
    Ok(())
}

pub fn validate_attachment_handle(handle: &str) -> Result<(), LiteRtError> {
    let h = handle.trim();
    if h.is_empty() || h.contains("..") || h.starts_with('/') || h.contains(":\\") {
        return Err(LiteRtError::AttachmentRejected(handle.into()));
    }
    if !(h.starts_with("runtime://attach/") || h.starts_with("att_")) {
        return Err(LiteRtError::AttachmentRejected(handle.into()));
    }
    Ok(())
}

pub fn select_provider(
    feature_enabled: bool,
    requested: &str,
    discovered: &[ProviderCapability],
    remote_consented: bool,
) -> Result<ProviderSelection, LiteRtError> {
    if !feature_enabled {
        return Err(LiteRtError::FeatureDisabled);
    }
    forbid_direct_backend(requested)?;

    let local = discovered.iter().find(|p| {
        p.id == requested && matches!(p.health, ProviderHealth::Ready | ProviderHealth::Degraded)
    });

    if let Some(cap) = local {
        return Ok(ProviderSelection {
            schema: LITERT_PROVIDER_SCHEMA_V1.into(),
            feature_enabled: true,
            requested_provider: requested.into(),
            effective_provider: cap.id.clone(),
            path: InferencePath::RuntimeLocal,
            device: cap.device.clone(),
            health: cap.health,
            fallback_reason: cap.degraded_reason.clone(),
            remote_fallback_requires_consent: true,
            remote_fallback_consented: remote_consented,
            queue_depth: 0,
            cancel_supported: true,
        });
    }

    // Remote fallback only with consent; still Runtime-mediated.
    if remote_consented {
        return Ok(ProviderSelection {
            schema: LITERT_PROVIDER_SCHEMA_V1.into(),
            feature_enabled: true,
            requested_provider: requested.into(),
            effective_provider: "runtime-remote".into(),
            path: InferencePath::RuntimeRemote,
            device: "remote".into(),
            health: ProviderHealth::Degraded,
            fallback_reason: Some("local_unavailable_consented_remote".into()),
            remote_fallback_requires_consent: true,
            remote_fallback_consented: true,
            queue_depth: 0,
            cancel_supported: true,
        });
    }

    if discovered.is_empty() {
        return Err(LiteRtError::Unavailable("no_providers".into()));
    }
    Err(LiteRtError::ConsentRequired)
}

pub fn authorize_request(
    selection: &ProviderSelection,
    request: &InferenceRequest,
) -> Result<(), LiteRtError> {
    if !selection.feature_enabled {
        return Err(LiteRtError::FeatureDisabled);
    }
    if selection.path == InferencePath::Blocked {
        return Err(LiteRtError::Unavailable("blocked".into()));
    }
    if request.prompt_tokens > request.context_limit {
        return Err(LiteRtError::ContextLimit {
            used: request.prompt_tokens,
            limit: request.context_limit,
        });
    }
    for handle in &request.attachment_handles {
        validate_attachment_handle(handle)?;
    }
    if selection.path == InferencePath::RuntimeRemote && !selection.remote_fallback_consented {
        return Err(LiteRtError::ConsentRequired);
    }
    Ok(())
}

pub fn receipt_for(
    request: &InferenceRequest,
    selection: &ProviderSelection,
    cancelled: bool,
    status: impl Into<String>,
    reason: Option<String>,
) -> InferenceReceipt {
    InferenceReceipt {
        schema: LITERT_PROVIDER_SCHEMA_V1.into(),
        request_id: request.request_id.clone(),
        attempted_provider: selection.requested_provider.clone(),
        effective_provider: selection.effective_provider.clone(),
        path: selection.path,
        cancelled,
        status: status.into(),
        reason_code: reason,
        redacted: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_ready() -> ProviderCapability {
        ProviderCapability {
            id: "litert-lm".into(),
            device: "cpu".into(),
            multimodal: false,
            max_context_tokens: 8192,
            health: ProviderHealth::Ready,
            degraded_reason: None,
        }
    }

    #[test]
    fn feature_flag_defaults_off() {
        assert!(!feature_enabled_from_env(None));
        assert!(!feature_enabled_from_env(Some("0")));
        assert!(feature_enabled_from_env(Some("1")));
    }

    #[test]
    fn never_opens_model_paths_or_direct_backend() {
        assert!(forbid_direct_backend("/models/foo.gguf").is_err());
        assert!(forbid_direct_backend("C:\\models\\x.tflite").is_err());
        assert!(forbid_direct_backend("litert-native:0").is_err());
        assert!(forbid_direct_backend("runtime://provider/litert-lm").is_ok());
    }

    #[test]
    fn selects_runtime_local_when_discovered() {
        let sel = select_provider(
            true,
            "runtime://provider/litert-lm",
            &[ProviderCapability {
                id: "runtime://provider/litert-lm".into(),
                ..local_ready()
            }],
            false,
        )
        .unwrap();
        assert_eq!(sel.path, InferencePath::RuntimeLocal);
        assert_eq!(sel.effective_provider, "runtime://provider/litert-lm");
        assert!(sel.cancel_supported);
    }

    #[test]
    fn remote_fallback_requires_consent() {
        let err = select_provider(true, "runtime://provider/missing", &[local_ready()], false)
            .unwrap_err();
        assert_eq!(err, LiteRtError::ConsentRequired);
        let sel =
            select_provider(true, "runtime://provider/missing", &[local_ready()], true).unwrap();
        assert_eq!(sel.path, InferencePath::RuntimeRemote);
    }

    #[test]
    fn context_limit_checked_before_send() {
        let sel = select_provider(
            true,
            "runtime://provider/litert-lm",
            &[ProviderCapability {
                id: "runtime://provider/litert-lm".into(),
                ..local_ready()
            }],
            false,
        )
        .unwrap();
        let req = InferenceRequest {
            request_id: "r1".into(),
            session_id: "s1".into(),
            provider_id: sel.effective_provider.clone(),
            prompt_tokens: 9000,
            context_limit: 8192,
            attachment_handles: vec![],
        };
        assert!(matches!(
            authorize_request(&sel, &req),
            Err(LiteRtError::ContextLimit { .. })
        ));
    }

    #[test]
    fn attachment_escape_rejected() {
        assert!(validate_attachment_handle("../etc/passwd").is_err());
        assert!(validate_attachment_handle("runtime://attach/img1").is_ok());
    }

    #[test]
    fn receipt_is_redacted_by_default() {
        let sel = select_provider(
            true,
            "runtime://provider/litert-lm",
            &[ProviderCapability {
                id: "runtime://provider/litert-lm".into(),
                ..local_ready()
            }],
            false,
        )
        .unwrap();
        let req = InferenceRequest {
            request_id: "r1".into(),
            session_id: "s1".into(),
            provider_id: sel.effective_provider.clone(),
            prompt_tokens: 10,
            context_limit: 100,
            attachment_handles: vec![],
        };
        let rec = receipt_for(&req, &sel, true, "cancelled", Some("user_cancel".into()));
        assert!(rec.redacted);
        assert!(rec.cancelled);
    }

    #[test]
    fn disabled_feature_is_removable() {
        assert_eq!(
            select_provider(false, "runtime://provider/x", &[], false).unwrap_err(),
            LiteRtError::FeatureDisabled
        );
    }
}
