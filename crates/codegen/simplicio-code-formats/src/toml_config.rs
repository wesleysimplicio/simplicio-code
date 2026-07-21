use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    #[serde(default = "default_handshake_timeout_ms")]
    pub handshake_timeout_ms: u64,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self { Self { handshake_timeout_ms: default_handshake_timeout_ms(), max_file_bytes: default_max_file_bytes() } }
}

fn default_handshake_timeout_ms() -> u64 { 2_000 }
fn default_max_file_bytes() -> u64 { 16 * 1024 * 1024 }

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("invalid TOML: {0}")]
    Parse(String),
    #[error("unsupported config schema version {0}")]
    Version(u32),
    #[error("unknown configuration key '{0}'")]
    UnknownKey(String),
}

/// Parses the human-authored Code configuration with a strict top-level and
/// runtime-key policy. Unknown keys fail closed instead of silently becoming
/// dead configuration.
pub fn parse_code_config(input: &str) -> Result<CodeConfig, ConfigError> {
    let value: toml::Value = toml::from_str(input).map_err(|e| ConfigError::Parse(e.to_string()))?;
    let root = value.as_table().ok_or_else(|| ConfigError::Parse("root must be a table".into()))?;
    for key in root.keys() {
        if !matches!(key.as_str(), "schema_version" | "runtime") { return Err(ConfigError::UnknownKey(key.clone())); }
    }
    if let Some(runtime) = root.get("runtime") {
        let table = runtime.as_table().ok_or_else(|| ConfigError::Parse("runtime must be a table".into()))?;
        for key in table.keys() {
            if !matches!(key.as_str(), "handshake_timeout_ms" | "max_file_bytes") { return Err(ConfigError::UnknownKey(format!("runtime.{key}"))); }
        }
    }
    let config: CodeConfig = value.try_into().map_err(|e| ConfigError::Parse(e.to_string()))?;
    if config.schema_version != 1 { return Err(ConfigError::Version(config.schema_version)); }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_toml_defaults_and_rejects_unknown_keys() {
        let config = parse_code_config("schema_version = 1\n[runtime]\n").unwrap();
        assert_eq!(config.runtime.handshake_timeout_ms, 2_000);
        assert!(matches!(parse_code_config("schema_version = 1\nunknown = true\n"), Err(ConfigError::UnknownKey(_))));
    }
}
