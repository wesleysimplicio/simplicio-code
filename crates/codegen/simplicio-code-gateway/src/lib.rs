//! Contracts and local test doubles for Simplicio Code identity and inference.
//!
//! This crate deliberately has no provider-specific knowledge. The client
//! speaks only to a configured Simplicio endpoint, keeps access tokens in
//! memory, stores only refresh tokens in the operating-system credential
//! store, and refuses inference without a current entitlement.

mod auth;
mod gateway;
mod redaction;

pub use auth::{
    AuthEndpoints, AuthError, AuthSession, AuthState, DeviceAuthorization, Entitlement,
    FakeIdentity, IdentityClient, MemorySecretStore, OsSecretStore, Secret, SecretStore,
    SecretString, TokenResponse,
};
pub use gateway::{
    ChatMessage, ChatRequest, FakeGateway, GatewayError, GatewayEvent, GatewayLimits, GatewayModel,
    GatewayUsage, PrivateGateway, ToolCall, ToolDefinition, parse_sse_events,
};
pub use redaction::{RedactedDiagnostics, redact_diagnostics};

/// The only model name a Simplicio Code client may advertise.
pub const PUBLIC_MODEL_ID: &str = "simplicio-1";

/// Contract paths shared by the HTTP implementation and local fakes.
pub mod paths {
    pub const DEVICE_AUTHORIZE: &str = "/v1/code/auth/device/authorize";
    pub const DEVICE_TOKEN: &str = "/v1/code/auth/device/token";
    pub const REFRESH: &str = "/v1/code/auth/token/refresh";
    pub const REVOKE: &str = "/v1/code/auth/session/revoke";
    pub const ENTITLEMENT: &str = "/v1/code/entitlement";
    pub const MODELS: &str = "/v1/code/models";
    pub const CHAT_COMPLETIONS: &str = "/v1/code/chat/completions";
    pub const USAGE: &str = "/v1/code/usage";
}
