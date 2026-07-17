//! OAuth configuration types for MCP servers.
//!
//! Constructed by the host's TOML parsing (`McpServerConfig::oauth_config`)
//! and consumed by [`crate::oauth`].

use std::collections::HashMap;

/// OAuth configuration extracted from an MCP server's config.
///
/// Travels alongside `acp::McpServer` (which can't be extended since it's
/// an external crate type). Keyed by server name in [`McpOAuthConfigMap`].
#[derive(Clone, Default)]
pub struct McpOAuthConfig {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub callback_port: Option<u16>,
}

/// Manual `Debug` impl: `client_secret` is a raw OAuth client secret. A
/// derived `Debug` would print it verbatim on any `{:?}` of this config
/// (e.g. logging the resolved MCP server config for diagnostics).
/// `client_id` is intentionally left un-redacted -- OAuth client IDs are
/// public identifiers, not secrets.
impl std::fmt::Debug for McpOAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthConfig")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("scopes", &self.scopes)
            .field("callback_port", &self.callback_port)
            .finish()
    }
}

impl McpOAuthConfig {
    pub fn is_configured(&self) -> bool {
        self.client_id.is_some()
    }
}

/// Per-server OAuth configuration map, keyed by MCP server name.
pub type McpOAuthConfigMap = HashMap<String, McpOAuthConfig>;

#[cfg(test)]
mod tests {
    use super::*;

    /// `client_secret` must never appear in `{:?}` output.
    #[test]
    fn debug_never_prints_raw_client_secret() {
        const CANARY_SECRET: &str = "canary-oauth-client-secret-00000000";

        let config = McpOAuthConfig {
            client_id: Some("public-client-id".to_string()),
            client_secret: Some(CANARY_SECRET.to_string()),
            scopes: Some(vec!["read".to_string()]),
            callback_port: Some(8080),
        };

        let debug_output = format!("{config:?}");
        assert!(
            !debug_output.contains(CANARY_SECRET),
            "McpOAuthConfig Debug leaked the raw client_secret: {debug_output}"
        );
        assert!(debug_output.contains("<redacted>"));
        assert!(debug_output.contains("public-client-id"));
    }
}
