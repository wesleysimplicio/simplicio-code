use chrono::Utc;
use futures_util::{Stream, StreamExt};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{pin::Pin, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{AuthError, AuthSession, PUBLIC_MODEL_ID, SecretStore, paths, redact_diagnostics};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("private gateway is not configured")]
    NotConfigured,
    #[error("private gateway endpoint is invalid")]
    InvalidEndpoint,
    #[error("private gateway rejected the request: {0}")]
    Rejected(String),
    #[error("private gateway limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("private gateway returned {status}: {message}")]
    Server { status: StatusCode, message: String },
    #[error("private gateway stream is malformed: {0}")]
    Protocol(String),
    #[error("private gateway request was cancelled")]
    Cancelled,
    #[error("private gateway transport failed: {0}")]
    Http(#[from] reqwest::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayModel {
    pub id: String,
    pub display_name: String,
    pub context_window: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayUsage {
    pub request_tokens: u64,
    pub response_tokens: u64,
    pub remaining_tokens: u64,
    pub remaining_tool_calls: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayLimits {
    pub max_request_tokens: u64,
    pub max_tool_calls: u32,
}

impl GatewayLimits {
    pub fn enforce(&self, request: &ChatRequest) -> Result<(), GatewayError> {
        if request.estimated_tokens > self.max_request_tokens { return Err(GatewayError::LimitExceeded(format!("request tokens {} > {}", request.estimated_tokens, self.max_request_tokens))); }
        if request.tools.len() > self.max_tool_calls as usize { return Err(GatewayError::LimitExceeded(format!("tool calls {} > {}", request.tools.len(), self.max_tool_calls))); }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub stream: bool,
    pub estimated_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>, estimated_tokens: u64) -> Self {
        Self { model: PUBLIC_MODEL_ID.into(), messages, tools: Vec::new(), stream: true, estimated_tokens, request_id: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayEvent {
    pub id: Option<String>,
    pub text_delta: Option<String>,
    pub tool_call: Option<ToolCall>,
    pub usage: Option<GatewayUsage>,
    pub done: bool,
}

pub type GatewayStream = Pin<Box<dyn Stream<Item = Result<GatewayEvent, GatewayError>> + Send>>;

pub struct PrivateGateway<S: SecretStore> {
    http: reqwest::Client,
    base_url: Url,
    session: Arc<AuthSession<S>>,
}

impl<S: SecretStore + 'static> PrivateGateway<S> {
    pub fn new(base_url: Url, session: Arc<AuthSession<S>>) -> Result<Self, GatewayError> {
        Self::with_http_client(base_url, session, reqwest::Client::new())
    }

    /// Same validation as [`Self::new`] but with a caller-supplied
    /// `reqwest::Client` — used by tests to set a short timeout so the
    /// `Http`/timeout error path can be exercised deterministically
    /// against a slow mock server instead of racing a real one.
    pub fn with_http_client(base_url: Url, session: Arc<AuthSession<S>>, http: reqwest::Client) -> Result<Self, GatewayError> {
        let loopback = base_url.host_str().is_some_and(|h| h == "localhost" || h == "127.0.0.1");
        if (base_url.scheme() != "https" && !(base_url.scheme() == "http" && loopback)) || base_url.username() != "" || base_url.password().is_some() { return Err(GatewayError::InvalidEndpoint); }
        Ok(Self { http, base_url, session })
    }

    fn url(&self, path: &str) -> Url {
        let mut url = self.base_url.clone();
        url.set_path(path);
        url.set_query(None);
        url
    }

    async fn token(&self) -> Result<String, GatewayError> {
        Ok(self.session.access_token(Utc::now())?.expose().to_owned())
    }

    pub async fn models(&self) -> Result<Vec<GatewayModel>, GatewayError> {
        let response = self.http.get(self.url(paths::MODELS)).bearer_auth(self.token().await?).send().await?;
        decode_json(response).await
    }

    pub async fn usage(&self) -> Result<GatewayUsage, GatewayError> {
        let response = self.http.get(self.url(paths::USAGE)).bearer_auth(self.token().await?).send().await?;
        decode_json(response).await
    }

    pub async fn chat_stream(&self, mut request: ChatRequest, limits: GatewayLimits, cancel: CancellationToken) -> Result<GatewayStream, GatewayError> {
        if request.model != PUBLIC_MODEL_ID { return Err(GatewayError::Rejected(format!("only {PUBLIC_MODEL_ID} is public"))); }
        limits.enforce(&request)?;
        request.stream = true;
        let response = self.http.post(self.url(paths::CHAT_COMPLETIONS)).bearer_auth(self.token().await?).json(&request).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(GatewayError::Server { status, message: redact_diagnostics(&serde_json::json!({"error": body})).message });
        }
        let mut bytes = response.bytes_stream();
        let stream = async_stream::try_stream! {
            let mut pending = Vec::new();
            loop {
                // `?` inside `tokio::select!` branches doesn't compose with
                // `async_stream::try_stream!`'s generator translation (the
                // select branches are not directly in the macro's body), so
                // each branch here produces a plain `Result` value first and
                // the `?`/`break` happens afterward, outside `select!`.
                let next = tokio::select! {
                    _ = cancel.cancelled() => Err(GatewayError::Cancelled),
                    item = bytes.next() => match item {
                        Some(Ok(chunk)) => Ok(Some(chunk)),
                        Some(Err(e)) => Err(GatewayError::Http(e)),
                        None => Ok(None),
                    },
                };
                let chunk = match next? {
                    Some(chunk) => chunk,
                    None => break,
                };
                pending.extend_from_slice(&chunk);
                let (events, rest) = parse_sse_events(&pending)?;
                pending = rest;
                for event in events { yield event; }
            }
            if !pending.is_empty() { let (events, rest) = parse_sse_events(&[pending.as_slice(), b"\n\n"].concat())?; if !rest.is_empty() { Err(GatewayError::Protocol("unterminated SSE frame".into()))?; } for event in events { yield event; } }
        };
        Ok(Box::pin(stream))
    }
}

pub fn parse_sse_events(input: &[u8]) -> Result<(Vec<GatewayEvent>, Vec<u8>), GatewayError> {
    let mut events = Vec::new();
    let mut offset = 0;
    while let Some(end) = input[offset..].windows(2).position(|w| w == b"\n\n") {
        let end = offset + end;
        let frame = &input[offset..end];
        let data = frame.split(|b| *b == b'\n').filter_map(|line| line.strip_prefix(b"data:")).map(|line| String::from_utf8_lossy(line).trim().to_owned()).collect::<Vec<_>>().join("\n");
        if data == "[DONE]" { events.push(GatewayEvent { id: None, text_delta: None, tool_call: None, usage: None, done: true }); }
        else if !data.is_empty() { events.push(serde_json::from_str(&data).map_err(|e| GatewayError::Protocol(e.to_string()))?); }
        offset = end + 2;
    }
    Ok((events, input[offset..].to_vec()))
}

async fn decode_json<T: for<'de> Deserialize<'de>>(response: reqwest::Response) -> Result<T, GatewayError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GatewayError::Server { status, message: redact_diagnostics(&serde_json::json!({"error": body})).message });
    }
    response.json().await.map_err(|e| GatewayError::Protocol(e.to_string()))
}

/// In-process fake used when staging is unavailable. It exposes the same
/// public contract without opening a socket or retaining prompt contents.
pub struct FakeGateway<S: SecretStore> {
    session: Arc<AuthSession<S>>,
    cancelled: AtomicBool,
}

impl<S: SecretStore> FakeGateway<S> {
    pub fn new(session: Arc<AuthSession<S>>) -> Self { Self { session, cancelled: AtomicBool::new(false) } }
    pub fn models(&self) -> Result<Vec<GatewayModel>, GatewayError> {
        self.session.entitlement(Utc::now())?;
        Ok(vec![GatewayModel { id: PUBLIC_MODEL_ID.into(), display_name: "Simplicio-1".into(), context_window: 262_144 }])
    }
    pub fn stream(&self, request: &ChatRequest, limits: GatewayLimits) -> Result<Vec<GatewayEvent>, GatewayError> {
        self.session.access_token(Utc::now())?;
        if request.model != PUBLIC_MODEL_ID { return Err(GatewayError::Rejected("model is not public".into())); }
        limits.enforce(request)?;
        if self.cancelled.load(Ordering::Relaxed) { return Err(GatewayError::Cancelled); }
        Ok(vec![GatewayEvent { id: Some("fake-request".into()), text_delta: Some("fake response".into()), tool_call: None, usage: Some(GatewayUsage { request_tokens: request.estimated_tokens, response_tokens: 2, remaining_tokens: limits.max_request_tokens.saturating_sub(request.estimated_tokens), remaining_tool_calls: limits.max_tool_calls.saturating_sub(request.tools.len() as u32) }), done: false }, GatewayEvent { id: Some("fake-request".into()), text_delta: None, tool_call: None, usage: None, done: true }])
    }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Relaxed); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entitlement, MemorySecretStore, SecretString, TokenResponse};
    use chrono::{Duration, Utc};

    fn session() -> Arc<AuthSession<MemorySecretStore>> {
        let store = Arc::new(MemorySecretStore::new());
        let session = Arc::new(AuthSession::new(store));
        let now = Utc::now();
        session.install(TokenResponse { access_token: SecretString::new("access"), refresh_token: Some(SecretString::new("refresh")), expires_in: 3600, token_type: "Bearer".into() }, Entitlement { plan: "pro".into(), expires_at: now + Duration::hours(1), max_request_tokens: 100, max_tool_calls: 2 }, now).unwrap();
        session
    }

    #[test]
    fn parser_handles_partial_sse_frames_and_done() {
        let first = b"data: {\"id\":\"r\",\"text_delta\":\"hi\",\"tool_call\":null,\"usage\":null,\"done\":false}\n";
        let (events, rest) = parse_sse_events(first).unwrap();
        assert!(events.is_empty());
        let (events, _rest) = parse_sse_events(&[rest, b"\ndata: [DONE]\n\n".to_vec()].concat()).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events[1].done);
    }

    #[test]
    fn fake_gateway_supports_public_model_and_limits() {
        let fake = FakeGateway::new(session());
        assert_eq!(fake.models().unwrap()[0].id, PUBLIC_MODEL_ID);
        let mut request = ChatRequest::new(vec![], 101);
        assert!(matches!(fake.stream(&request, GatewayLimits { max_request_tokens: 100, max_tool_calls: 2 }), Err(GatewayError::LimitExceeded(_))));
        request.estimated_tokens = 1;
        assert_eq!(fake.stream(&request, GatewayLimits { max_request_tokens: 100, max_tool_calls: 2 }).unwrap().len(), 2);
        fake.cancel();
        assert!(matches!(fake.stream(&request, GatewayLimits { max_request_tokens: 100, max_tool_calls: 2 }), Err(GatewayError::Cancelled)));
    }
}
