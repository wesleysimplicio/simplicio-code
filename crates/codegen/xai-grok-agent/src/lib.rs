//! Agent builder, definition parsing, and system prompt assembly.
//!
//! This crate extracts a first-class `Agent` type from `xai-grok-shell`.
//! An `Agent` bundles tools, system prompt, system-reminder policy,
//! compaction policy, and model configuration into a single, portable
//! object that any host can consume.

pub mod agent;
pub mod agent_host;
pub mod builder;
pub mod compaction;
pub mod config;
pub mod discovery;
pub mod error;
pub mod plugins;
pub mod prompt;
pub mod repo;
pub mod system_reminder;
pub mod timing;

pub use agent::Agent;
pub use agent_host::SimplicioAgentCoordinator;
pub use builder::AgentBuilder;
pub use compaction::CompactionPolicy;
pub use config::AgentDefinition;
pub use config::preset_names;
pub use config::toolset_for_preset;
pub use config::workspace_grok_build_toolset;
pub use error::AgentBuildError;
pub use prompt::context::{DEFAULT_SYSTEM_PROMPT_LABEL, PromptContext};
pub use system_reminder::ReminderPolicy;

/// Return the model/provider selected by the installed Hermes-derived Agent.
///
/// Code uses this only for its shared UI label. Turn execution remains behind
/// AgentHost, so the label cannot silently become a second provider config.
pub fn configured_model_label() -> Option<String> {
    let path = dirs::home_dir()?
        .join(".simplicio_agent")
        .join("config.yaml");
    let contents = std::fs::read_to_string(path).ok()?;
    let root: serde_yaml::Value = serde_yaml::from_str(&contents).ok()?;
    let model = root.get("model")?;
    let model_name = model
        .get("default")
        .or_else(|| model.get("name"))
        .and_then(serde_yaml::Value::as_str)?;
    let provider = model
        .get("provider")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("configured provider");
    Some(format!("{model_name} · {provider}"))
}
