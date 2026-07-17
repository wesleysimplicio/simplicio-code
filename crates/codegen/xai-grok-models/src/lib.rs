//! Default model IDs loaded from `default_models.json` at runtime.
//! Edit that JSON file to change them.
//!
//! At runtime each model is resolved via:
//!   CLI flag > ENV var > config.toml > remote settings > these defaults

use std::sync::LazyLock;

/// The raw JSON, embedded at compile time. Re-exported through the
/// `xai_grok_shell::models` facade and consumed by `agent::config`, so it must
/// be `pub` (was `pub(crate)` when this lived inside the shell crate).
pub const DEFAULT_MODELS_JSON: &str = include_str!("../default_models.json");

#[derive(serde::Deserialize)]
struct DefaultModels {
    default: String,
    /// Falls back to `default` if not specified in JSON.
    web_search: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    image_description: Option<String>,
    /// Falls back to `default` if not specified in JSON.
    session_summary: Option<String>,
    models: Vec<DefaultModelEntry>,
}

#[derive(serde::Deserialize)]
struct DefaultModelEntry {
    /// Stable unique identifier for this catalog entry. When present, this is
    /// the key `default`/`web_search`/etc. must reference. Falls back to
    /// `model` when absent — mirrors `ModelEntryConfig::id` in
    /// `xai-grok-shell::agent::config`, whose `build_prefetched_map`/
    /// `default_models` key the runtime catalog the same way.
    id: Option<String>,
    model: String,
}

impl DefaultModelEntry {
    /// The catalog key for this entry: `id` if set, else `model`.
    fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.model)
    }
}

static DEFAULTS: LazyLock<DefaultModels> = LazyLock::new(|| {
    let defaults: DefaultModels = serde_json::from_str(DEFAULT_MODELS_JSON)
        .expect("default_models.json: invalid JSON or missing 'default' field");
    validate(&defaults);
    defaults
});

/// Baked-in JSON — a mismatch here is a developer error, not a runtime
/// condition. `default` (and the optional per-purpose overrides) must
/// reference a catalog entry by its `id` (falling back to `model` when an
/// entry has no `id`), matching the key scheme used when the runtime model
/// catalog is built from this same file.
fn validate(defaults: &DefaultModels) {
    let keys: Vec<&str> = defaults.models.iter().map(DefaultModelEntry::key).collect();

    let check = |field: &str, value: &str| {
        assert!(
            keys.contains(&value),
            "default_models.json: '{field}' is '{value}' but 'models' array only has {keys:?}",
        );
    };

    check("default", &defaults.default);
    if let Some(v) = &defaults.web_search {
        check("web_search", v);
    }
    if let Some(v) = &defaults.image_description {
        check("image_description", v);
    }
    if let Some(v) = &defaults.session_summary {
        check("session_summary", v);
    }
}

/// Primary model for coding tasks and general fallback.
pub fn default_model() -> &'static str {
    &DEFAULTS.default
}

/// Model for web search tool synthesis. Falls back to default model.
pub fn default_web_search_model() -> &'static str {
    DEFAULTS.web_search.as_deref().unwrap_or(&DEFAULTS.default)
}

/// Model for image describe. Falls back to default model.
pub fn default_image_description_model() -> &'static str {
    DEFAULTS
        .image_description
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}

/// Model for session title generation. Falls back to default model.
pub fn default_session_summary_model() -> &'static str {
    DEFAULTS
        .session_summary
        .as_deref()
        .unwrap_or(&DEFAULTS.default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the panic this crate shipped with: `default` in
    /// `default_models.json` is an entry's `id` (e.g. "simplicio-1"), not its
    /// `model` routing slug (e.g. "tencent/hy3:free"). The validator used to
    /// compare `default` against the `model` field only, so any entry that
    /// set `id` (the normal case) panicked at first use of `default_model()`
    /// et al. This exercises the real, shipped `default_models.json` end to
    /// end — the same file baked into the release binary — through every
    /// public accessor.
    #[test]
    fn real_shipped_json_resolves_default_models_without_panicking() {
        assert_eq!(default_model(), "simplicio-1");
        assert_eq!(default_web_search_model(), "simplicio-1");
        assert_eq!(default_image_description_model(), "simplicio-1");
        assert_eq!(default_session_summary_model(), "simplicio-1");
    }

    /// Every `models` entry's key (its `id`, or `model` when `id` is absent)
    /// must be unique, and `default`/`web_search`/`image_description`/
    /// `session_summary` must each resolve to one of those keys. This is the
    /// consistency check `validate()` runs on the real file at startup;
    /// re-run it here so a future edit to `default_models.json` that breaks
    /// it fails `cargo test` instead of panicking at runtime.
    #[test]
    fn real_shipped_json_is_internally_consistent() {
        let defaults: DefaultModels = serde_json::from_str(DEFAULT_MODELS_JSON)
            .expect("default_models.json should be valid JSON matching the schema");

        let keys: Vec<&str> = defaults.models.iter().map(DefaultModelEntry::key).collect();
        let mut unique_keys = keys.clone();
        unique_keys.sort_unstable();
        unique_keys.dedup();
        assert_eq!(
            keys.len(),
            unique_keys.len(),
            "default_models.json: 'models' entries have duplicate keys (id, or model when id is absent): {keys:?}"
        );

        // Does not panic: this is exactly what `validate()` asserts.
        validate(&defaults);
    }

    fn entry(id: Option<&str>, model: &str) -> DefaultModelEntry {
        DefaultModelEntry {
            id: id.map(str::to_string),
            model: model.to_string(),
        }
    }

    #[test]
    fn key_prefers_id_over_model() {
        assert_eq!(
            entry(Some("simplicio-1"), "tencent/hy3:free").key(),
            "simplicio-1"
        );
    }

    #[test]
    fn key_falls_back_to_model_when_id_absent() {
        assert_eq!(entry(None, "grok-4").key(), "grok-4");
    }

    #[test]
    #[should_panic(expected = "'default' is 'simplicio-1' but 'models' array only has")]
    fn validate_panics_when_default_references_model_slug_not_id() {
        // Reproduces the original bug: `default` names an entry's `id`, but
        // the only catalog entry exposes a *different* `model` slug and no
        // matching `id` — so 'default' can't resolve to any key.
        let defaults = DefaultModels {
            default: "simplicio-1".to_string(),
            web_search: None,
            image_description: None,
            session_summary: None,
            models: vec![entry(None, "tencent/hy3:free")],
        };
        validate(&defaults);
    }

    #[test]
    fn validate_accepts_default_matching_entry_id() {
        let defaults = DefaultModels {
            default: "simplicio-1".to_string(),
            web_search: None,
            image_description: None,
            session_summary: None,
            models: vec![entry(Some("simplicio-1"), "tencent/hy3:free")],
        };
        validate(&defaults); // must not panic
    }
}
