//! Versioned, transport-only client boundary for the Simplicio Loop Hub.
//!
//! Code owns interactive UX state, but it must not own a workflow scheduler,
//! resource queue, Runtime/Mapper process, or inference pool. This module is
//! deliberately a thin client: it discovers an already-running Hub endpoint,
//! negotiates ownership/capacity, and forwards submit/progress/cancel/resume
//! requests. A transport implementation is supplied by the product adapter;
//! the crate never spawns a daemon or performs local effects.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    env,
    sync::{Arc, LazyLock, Mutex, Weak},
};

pub const LOOP_HUB_CLIENT_SCHEMA: &str = "simplicio.loop-hub-client/v1";
pub const LOOP_HUB_PROTOCOL: &str = "simplicio.loop-hub/v1";
const HUB_ENDPOINT_ENV: &str = "SIMPLICIO_LOOP_HUB_ENDPOINT";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HubMode {
    Auto,
    Hub,
    Required,
    Standalone,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SharedService {
    Runtime,
    Mapper,
    Scheduler,
    Inference,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceOwner {
    LoopHub,
    CodeProcess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceOwnership {
    pub name: SharedService,
    pub owner: ServiceOwner,
    #[serde(default)]
    pub process_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceHandle {
    pub id: String,
    pub capacity: u32,
    pub used: u32,
}

impl ResourceHandle {
    fn is_valid(&self) -> bool {
        !self.id.trim().is_empty() && self.used <= self.capacity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedResources {
    pub runtime: ResourceHandle,
    pub mapper: ResourceHandle,
    pub inference: ResourceHandle,
    /// The model engine is a single Runtime-owned actor. Tool/preprocessing
    /// capacity may be wider, but generation cannot be duplicated by Code.
    pub max_active_inference: u32,
    pub interactive_reserved: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueCapabilities {
    /// These bounds are enforced by the Hub resource queue. Code's prompt
    /// queue never stores resource work or performs admission itself.
    pub interactive_capacity: u32,
    pub background_capacity: u32,
    pub max_pending_interactive: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubHandshake {
    pub schema: String,
    pub protocol: String,
    pub hub_id: String,
    pub ready: bool,
    /// Compatible external Agent through which reasoning is routed. Code must
    /// never substitute its embedded loop for this production dependency.
    pub agent: AgentCapability,
    pub services: Vec<ServiceOwnership>,
    pub resources: SharedResources,
    pub queue: QueueCapabilities,
    /// The Hub must be the only scheduler owner. Code uses this to fail closed
    /// instead of silently creating a local fan-out loop.
    #[serde(default)]
    pub local_scheduler: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentCapability {
    pub agent_id: String,
    pub protocol: String,
    pub ready: bool,
}

impl HubHandshake {
    pub fn validate(&self) -> Result<(), HubError> {
        let mut errors = Vec::new();
        if self.schema != LOOP_HUB_CLIENT_SCHEMA {
            errors.push(format!(
                "unsupported Hub schema {}; expected {LOOP_HUB_CLIENT_SCHEMA}",
                self.schema
            ));
        }
        if self.protocol != LOOP_HUB_PROTOCOL {
            errors.push(format!(
                "unsupported Hub protocol {}; expected {LOOP_HUB_PROTOCOL}",
                self.protocol
            ));
        }
        if self.hub_id.trim().is_empty() {
            errors.push("Hub identity is empty".into());
        }
        if !self.ready {
            errors.push("Loop Hub is not ready".into());
        }
        if !self.agent.ready
            || self.agent.agent_id.trim().is_empty()
            || self.agent.protocol.trim().is_empty()
        {
            errors.push("a ready, versioned Simplicio Agent is required".into());
        }
        if self.local_scheduler {
            errors.push("Code cannot attach while a local scheduler is declared".into());
        }
        let mut owners = HashMap::new();
        for service in &self.services {
            if service.owner != ServiceOwner::LoopHub {
                errors.push(format!(
                    "{} must be owned by Loop Hub, got {:?}",
                    service_name(service.name),
                    service.owner
                ));
            }
            if service.process_id.as_deref().is_none_or(str::is_empty) {
                errors.push(format!(
                    "{} has no process identity",
                    service_name(service.name)
                ));
            }
            if owners.insert(service.name, service).is_some() {
                errors.push(format!(
                    "duplicate {} service declaration",
                    service_name(service.name)
                ));
            }
        }
        for service in [
            SharedService::Runtime,
            SharedService::Mapper,
            SharedService::Scheduler,
            SharedService::Inference,
        ] {
            if !owners.contains_key(&service) {
                errors.push(format!(
                    "missing {} service declaration",
                    service_name(service)
                ));
            }
        }
        if !self.resources.runtime.is_valid()
            || !self.resources.mapper.is_valid()
            || !self.resources.inference.is_valid()
        {
            errors.push("Hub returned an invalid shared resource handle".into());
        }
        if self.resources.interactive_reserved == 0 {
            errors.push("Hub returned no interactive capacity reservation".into());
        }
        if self.resources.max_active_inference != 1 {
            errors.push(format!(
                "Hub must expose exactly one active inference slot, got {}",
                self.resources.max_active_inference
            ));
        }
        if self.queue.interactive_capacity == 0
            || self.queue.background_capacity == 0
            || self.queue.max_pending_interactive == 0
        {
            errors.push("Hub returned an unbounded or empty queue capacity".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(HubError::Incompatible(errors.join("; ")))
        }
    }
}

fn service_name(service: SharedService) -> &'static str {
    match service {
        SharedService::Runtime => "runtime",
        SharedService::Mapper => "mapper",
        SharedService::Scheduler => "scheduler",
        SharedService::Inference => "inference",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubHandshakeRequest {
    pub schema: String,
    pub protocol: String,
    pub client_id: String,
    pub workspace_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HubClientConfig {
    pub mode: HubMode,
    pub client_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub endpoint: Option<String>,
}

impl HubClientConfig {
    pub fn new(
        mode: HubMode,
        client_id: impl Into<String>,
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            mode,
            client_id: client_id.into(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            endpoint: None,
        }
    }

    fn configured_endpoint(&self) -> Option<String> {
        normalize_endpoint(self.endpoint.clone())
    }

    fn validate(&self) -> Result<(), HubError> {
        for (name, value) in [
            ("client_id", &self.client_id),
            ("workspace_id", &self.workspace_id),
            ("session_id", &self.session_id),
        ] {
            if value.trim().is_empty() {
                return Err(HubError::InvalidRequest(format!(
                    "{name} must not be empty"
                )));
            }
        }
        Ok(())
    }
}

fn normalize_endpoint(endpoint: Option<String>) -> Option<String> {
    endpoint
        .map(|endpoint| endpoint.trim().to_owned())
        .filter(|endpoint| !endpoint.is_empty())
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error(
        "Loop Hub endpoint was not discovered; configure an endpoint or provide a discovery adapter"
    )]
    EndpointNotFound,
    #[error("Loop Hub is required but no compatible endpoint was discovered")]
    RequiredHubUnavailable,
    #[error("Loop Hub protocol error: {0}")]
    Protocol(String),
    #[error("Loop Hub is incompatible: {0}")]
    Incompatible(String),
    #[error("invalid Loop Hub request: {0}")]
    InvalidRequest(String),
    #[error("Loop Hub transport is unavailable: {0}")]
    TransportUnavailable(String),
}

/// Discovers an endpoint for an already-running Loop Hub.
///
/// The discovery implementation is supplied by the product adapter because
/// the executable endpoint contract is external to Code. Implementations must
/// only locate/reuse an endpoint; they must not spawn a daemon, scheduler,
/// Runtime, Mapper, worker, or inference process.
pub trait HubEndpointDiscovery: Send + Sync {
    fn discover(&self, config: &HubClientConfig) -> Result<Option<String>, HubError>;
}

/// Default discovery used by [`LoopHubClient::connect`].
///
/// This is intentionally limited to the explicit environment contract. A
/// product integration that discovers a per-user or per-machine socket should
/// implement [`HubEndpointDiscovery`] and call
/// [`LoopHubClient::connect_with_discovery`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvironmentHubEndpointDiscovery;

impl HubEndpointDiscovery for EnvironmentHubEndpointDiscovery {
    fn discover(&self, _: &HubClientConfig) -> Result<Option<String>, HubError> {
        Ok(normalize_endpoint(env::var(HUB_ENDPOINT_ENV).ok()))
    }
}

impl<F> HubEndpointDiscovery for F
where
    F: Fn(&HubClientConfig) -> Result<Option<String>, HubError> + Send + Sync,
{
    fn discover(&self, config: &HubClientConfig) -> Result<Option<String>, HubError> {
        self(config)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    Interactive,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InteractiveGoal {
    pub goal_id: String,
    pub turn_id: String,
    pub payload: Value,
    pub deadline_unix_ms: Option<u64>,
    pub budget_tokens: Option<u64>,
}

impl InteractiveGoal {
    pub fn new(goal_id: impl Into<String>, turn_id: impl Into<String>, payload: Value) -> Self {
        Self {
            goal_id: goal_id.into(),
            turn_id: turn_id.into(),
            payload,
            deadline_unix_ms: None,
            budget_tokens: None,
        }
    }

    fn validate(&self) -> Result<(), HubError> {
        if self.goal_id.trim().is_empty() || self.turn_id.trim().is_empty() {
            return Err(HubError::InvalidRequest(
                "goal_id and turn_id must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitRequest {
    pub schema: String,
    pub session_id: String,
    pub goal_id: String,
    pub turn_id: String,
    pub idempotency_key: String,
    pub priority: PriorityClass,
    pub deadline_unix_ms: Option<u64>,
    pub budget_tokens: Option<u64>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionState {
    Accepted,
    Queued,
    Throttled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdmissionReceipt {
    pub schema: String,
    pub workflow_id: String,
    pub state: AdmissionState,
    pub queue_position: Option<u32>,
    pub retry_after_ms: Option<u64>,
    pub receipt_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgressRequest {
    pub workflow_id: String,
    pub after_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub workflow_id: String,
    pub next_sequence: u64,
    pub events: Vec<ProgressEvent>,
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    Queued {
        sequence: u64,
        position: u32,
    },
    Started {
        sequence: u64,
        worker_id: String,
    },
    Output {
        sequence: u64,
        text: String,
    },
    Throttled {
        sequence: u64,
        retry_after_ms: u64,
    },
    Cancelled {
        sequence: u64,
        receipt_id: String,
    },
    Resumed {
        sequence: u64,
        receipt_id: String,
    },
    Completed {
        sequence: u64,
        receipt_id: String,
    },
    Failed {
        sequence: u64,
        message: String,
        receipt_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelRequest {
    pub workflow_id: String,
    pub reason: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeRequest {
    pub workflow_id: String,
    pub idempotency_key: String,
    pub checkpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleReceipt {
    pub schema: String,
    pub workflow_id: String,
    pub receipt_id: String,
    pub state: String,
}

/// The only effectful boundary in this module. Implementations belong to the
/// Loop Hub adapter and must use the Hub's queue, claims, Runtime, Mapper and
/// worker pools. This trait intentionally has no `spawn`, `exec`, or local
/// scheduler method.
pub trait HubTransport: Send + Sync {
    fn handshake(&self, request: &HubHandshakeRequest) -> Result<HubHandshake, HubError>;
    fn submit(&self, request: &SubmitRequest) -> Result<AdmissionReceipt, HubError>;
    fn progress(&self, request: &ProgressRequest) -> Result<ProgressSnapshot, HubError>;
    fn cancel(&self, request: &CancelRequest) -> Result<LifecycleReceipt, HubError>;
    fn resume(&self, request: &ResumeRequest) -> Result<LifecycleReceipt, HubError>;
}

pub trait HubTransportFactory: Send + Sync {
    fn connect(&self, endpoint: &str) -> Result<Arc<dyn HubTransport>, HubError>;
}

impl<F> HubTransportFactory for F
where
    F: Fn(&str) -> Result<Arc<dyn HubTransport>, HubError> + Send + Sync,
{
    fn connect(&self, endpoint: &str) -> Result<Arc<dyn HubTransport>, HubError> {
        self(endpoint)
    }
}

struct HubSession {
    endpoint: String,
    transport: Arc<dyn HubTransport>,
    handshake: HubHandshake,
}

static HUB_SESSIONS: LazyLock<Mutex<HashMap<String, Weak<HubSession>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A cloneable interactive client. Clones share one transport connection and
/// one negotiated Hub session; submit/cancel/resume remain remote operations.
#[derive(Clone)]
pub struct LoopHubClient {
    session: Arc<HubSession>,
    config: HubClientConfig,
}

/// A capability limited to interactive Loop Hub submission.
///
/// UI integrations can retain this handle without gaining access to the raw
/// [`HubTransport`] (and therefore cannot bypass request validation or submit
/// a different priority class). Clones retain the negotiated Hub session; they
/// do not open another socket or create local scheduling state.
#[derive(Clone)]
pub struct InteractiveHubTransport {
    session: Arc<HubSession>,
    session_id: String,
}

/// A cloneable reference to a Hub-owned shared service.
///
/// This is an identity/capacity handle, not a local service client. Keeping it
/// alive also keeps the negotiated Hub session alive, which makes it safe for
/// independently constructed Code surfaces to share Map and Runtime without
/// spawning either service in the Code process.
#[derive(Clone)]
pub struct SharedHubServiceHandle {
    session: Arc<HubSession>,
    service: SharedService,
    resource: ResourceHandle,
}

impl SharedHubServiceHandle {
    pub fn service(&self) -> SharedService {
        self.service
    }

    pub fn resource(&self) -> &ResourceHandle {
        &self.resource
    }

    pub fn hub_id(&self) -> &str {
        &self.session.handshake.hub_id
    }

    pub fn endpoint(&self) -> &str {
        &self.session.endpoint
    }

    /// Returns true when two service handles use the same negotiated Hub
    /// session, rather than merely advertising equal string identifiers.
    pub fn shares_session_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.session, &other.session)
    }
}

impl LoopHubClient {
    /// Connect using the environment-backed endpoint discovery contract.
    pub fn connect<F: HubTransportFactory>(
        config: HubClientConfig,
        factory: &F,
    ) -> Result<Option<Self>, HubError> {
        Self::connect_with_discovery(config, &EnvironmentHubEndpointDiscovery, factory)
    }

    /// Connect using a product-supplied endpoint discovery adapter.
    ///
    /// An explicitly configured endpoint wins over discovery. Discovery is
    /// skipped entirely for [`HubMode::Standalone`]. This method only
    /// negotiates an already-running Hub through `factory`; it never starts a
    /// local daemon or scheduler.
    pub fn connect_with_discovery<D: HubEndpointDiscovery, F: HubTransportFactory>(
        config: HubClientConfig,
        discovery: &D,
        factory: &F,
    ) -> Result<Option<Self>, HubError> {
        config.validate()?;
        if config.mode == HubMode::Standalone {
            return Ok(None);
        }
        let endpoint = match config.configured_endpoint() {
            Some(endpoint) => endpoint,
            None => normalize_endpoint(discovery.discover(&config)?).ok_or_else(|| {
                if config.mode == HubMode::Required {
                    HubError::RequiredHubUnavailable
                } else {
                    HubError::EndpointNotFound
                }
            })?,
        };
        // An external transport is attached to exactly one logical Code
        // session (the socket/pipe attach contract carries `session_id`).
        // Reuse it only for clones of that session. Other surfaces attach
        // independently to the same Hub endpoint and therefore cannot submit
        // work under a session that the Hub never admitted.
        let key = format!(
            "{}\0{}\0{}",
            endpoint, config.workspace_id, config.session_id
        );
        let mut sessions = HUB_SESSIONS
            .lock()
            .map_err(|_| HubError::Protocol("Hub session registry lock poisoned".into()))?;
        if let Some(session) = sessions.get(&key).and_then(Weak::upgrade) {
            return Ok(Some(Self { session, config }));
        }
        let transport = factory.connect(&endpoint)?;
        let handshake = transport.handshake(&HubHandshakeRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            protocol: LOOP_HUB_PROTOCOL.into(),
            client_id: config.client_id.clone(),
            workspace_id: config.workspace_id.clone(),
            session_id: config.session_id.clone(),
        })?;
        handshake.validate()?;
        let session = Arc::new(HubSession {
            endpoint,
            transport,
            handshake,
        });
        sessions.insert(key, Arc::downgrade(&session));
        Ok(Some(Self { session, config }))
    }

    pub fn endpoint(&self) -> &str {
        &self.session.endpoint
    }

    pub fn handshake(&self) -> &HubHandshake {
        &self.session.handshake
    }

    pub fn config(&self) -> &HubClientConfig {
        &self.config
    }

    /// Exposes the interactive-only transport capability for UI adapters.
    pub fn interactive_transport(&self) -> InteractiveHubTransport {
        InteractiveHubTransport {
            session: Arc::clone(&self.session),
            session_id: self.config.session_id.clone(),
        }
    }

    /// Returns the Runtime handle negotiated from the Hub handshake.
    pub fn shared_runtime_handle(&self) -> SharedHubServiceHandle {
        self.shared_service_handle(
            SharedService::Runtime,
            self.session.handshake.resources.runtime.clone(),
        )
    }

    /// Returns the Map/Mapper handle negotiated from the Hub handshake.
    pub fn shared_map_handle(&self) -> SharedHubServiceHandle {
        self.shared_service_handle(
            SharedService::Mapper,
            self.session.handshake.resources.mapper.clone(),
        )
    }

    fn shared_service_handle(
        &self,
        service: SharedService,
        resource: ResourceHandle,
    ) -> SharedHubServiceHandle {
        SharedHubServiceHandle {
            session: Arc::clone(&self.session),
            service,
            resource,
        }
    }

    pub fn submit_interactive(&self, goal: InteractiveGoal) -> Result<HubJob, HubError> {
        self.interactive_transport().submit(goal)
    }
}

impl InteractiveHubTransport {
    /// Submit one interactive turn through the already negotiated Hub queue.
    pub fn submit(&self, goal: InteractiveGoal) -> Result<HubJob, HubError> {
        goal.validate()?;
        // Length-prefix every component. Delimiter joining is ambiguous (for
        // example `a:b,c` and `a,b:c`) and can alias two user turns to the
        // same effect receipt. The Hub treats this opaque value as the stable
        // replay key, so it must be injective without restricting valid IDs.
        let idempotency_key = idempotency_key(&[&self.session_id, &goal.turn_id, &goal.goal_id]);
        let receipt = self.session.transport.submit(&SubmitRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            session_id: self.session_id.clone(),
            goal_id: goal.goal_id,
            turn_id: goal.turn_id,
            idempotency_key,
            priority: PriorityClass::Interactive,
            deadline_unix_ms: goal.deadline_unix_ms,
            budget_tokens: goal.budget_tokens,
            payload: goal.payload,
        })?;
        if receipt.schema != LOOP_HUB_CLIENT_SCHEMA
            || receipt.workflow_id.trim().is_empty()
            || receipt.receipt_id.trim().is_empty()
            || matches!(receipt.state, AdmissionState::Queued) && receipt.queue_position.is_none()
            || matches!(receipt.state, AdmissionState::Throttled)
                && receipt.retry_after_ms.is_none()
        {
            return Err(HubError::Protocol(
                "Hub returned an invalid admission receipt".into(),
            ));
        }
        Ok(HubJob {
            session: Arc::clone(&self.session),
            workflow_id: receipt.workflow_id.clone(),
            next_sequence: 0,
            admission: receipt,
        })
    }
}

pub struct HubJob {
    session: Arc<HubSession>,
    workflow_id: String,
    next_sequence: u64,
    admission: AdmissionReceipt,
}

impl HubJob {
    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    pub fn admission(&self) -> &AdmissionReceipt {
        &self.admission
    }

    pub fn poll(&mut self) -> Result<ProgressSnapshot, HubError> {
        let requested_sequence = self.next_sequence;
        let snapshot = self.session.transport.progress(&ProgressRequest {
            workflow_id: self.workflow_id.clone(),
            after_sequence: requested_sequence,
        })?;
        if snapshot.workflow_id != self.workflow_id
            || snapshot.next_sequence < requested_sequence
            || snapshot.events.iter().any(|event| {
                event_sequence(event) < requested_sequence
                    || event_sequence(event) >= snapshot.next_sequence
            })
        {
            return Err(HubError::Protocol(
                "Hub returned invalid, stale, or cross-workflow progress".into(),
            ));
        }
        self.next_sequence = snapshot.next_sequence;
        Ok(snapshot)
    }

    pub fn cancel(&self, reason: impl Into<String>) -> Result<LifecycleReceipt, HubError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(HubError::InvalidRequest(
                "cancel reason must not be empty".into(),
            ));
        }
        let receipt = self.session.transport.cancel(&CancelRequest {
            workflow_id: self.workflow_id.clone(),
            idempotency_key: idempotency_key(&[&self.workflow_id, "cancel"]),
            reason,
        })?;
        validate_lifecycle_receipt(&receipt, &self.workflow_id)?;
        Ok(receipt)
    }

    pub fn resume(&mut self, checkpoint: Option<String>) -> Result<LifecycleReceipt, HubError> {
        let receipt = self.session.transport.resume(&ResumeRequest {
            workflow_id: self.workflow_id.clone(),
            idempotency_key: idempotency_key(&[&self.workflow_id, "resume"]),
            checkpoint,
        })?;
        validate_lifecycle_receipt(&receipt, &self.workflow_id)?;
        self.next_sequence = 0;
        Ok(receipt)
    }
}

fn idempotency_key(parts: &[&str]) -> String {
    use std::fmt::Write;

    let capacity = parts.iter().map(|part| part.len() + 22).sum();
    let mut key = String::with_capacity(capacity);
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            key.push('|');
        }
        write!(key, "{}:{part}", part.len()).expect("writing to String cannot fail");
    }
    key
}

fn event_sequence(event: &ProgressEvent) -> u64 {
    match event {
        ProgressEvent::Queued { sequence, .. }
        | ProgressEvent::Started { sequence, .. }
        | ProgressEvent::Output { sequence, .. }
        | ProgressEvent::Throttled { sequence, .. }
        | ProgressEvent::Cancelled { sequence, .. }
        | ProgressEvent::Resumed { sequence, .. }
        | ProgressEvent::Completed { sequence, .. }
        | ProgressEvent::Failed { sequence, .. } => *sequence,
    }
}

fn validate_lifecycle_receipt(
    receipt: &LifecycleReceipt,
    workflow_id: &str,
) -> Result<(), HubError> {
    if receipt.schema != LOOP_HUB_CLIENT_SCHEMA
        || receipt.workflow_id != workflow_id
        || receipt.receipt_id.trim().is_empty()
        || receipt.state.trim().is_empty()
    {
        return Err(HubError::Protocol(
            "Hub returned an invalid lifecycle receipt".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeHub {
        handshakes: AtomicUsize,
        handshake_requests: Mutex<Vec<HubHandshakeRequest>>,
        submits: Mutex<Vec<SubmitRequest>>,
        progress_calls: Mutex<Vec<ProgressRequest>>,
    }

    fn handshake() -> HubHandshake {
        HubHandshake {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            protocol: LOOP_HUB_PROTOCOL.into(),
            hub_id: "hub-1".into(),
            ready: true,
            agent: AgentCapability {
                agent_id: "agent-1".into(),
                protocol: "simplicio.agent/v1".into(),
                ready: true,
            },
            services: [
                (SharedService::Runtime, "runtime-1"),
                (SharedService::Mapper, "mapper-1"),
                (SharedService::Scheduler, "scheduler-1"),
                (SharedService::Inference, "inference-1"),
            ]
            .into_iter()
            .map(|(name, process_id)| ServiceOwnership {
                name,
                owner: ServiceOwner::LoopHub,
                process_id: Some(process_id.into()),
            })
            .collect(),
            resources: SharedResources {
                runtime: ResourceHandle {
                    id: "r".into(),
                    capacity: 1,
                    used: 0,
                },
                mapper: ResourceHandle {
                    id: "m".into(),
                    capacity: 1,
                    used: 0,
                },
                inference: ResourceHandle {
                    id: "i".into(),
                    capacity: 2,
                    used: 0,
                },
                max_active_inference: 1,
                interactive_reserved: 1,
            },
            queue: QueueCapabilities {
                interactive_capacity: 1,
                background_capacity: 1,
                max_pending_interactive: 8,
            },
            local_scheduler: false,
        }
    }

    impl HubTransport for FakeHub {
        fn handshake(&self, request: &HubHandshakeRequest) -> Result<HubHandshake, HubError> {
            self.handshakes.fetch_add(1, Ordering::SeqCst);
            self.handshake_requests
                .lock()
                .unwrap()
                .push(request.clone());
            Ok(handshake())
        }

        fn submit(&self, request: &SubmitRequest) -> Result<AdmissionReceipt, HubError> {
            self.submits.lock().unwrap().push(request.clone());
            Ok(AdmissionReceipt {
                schema: LOOP_HUB_CLIENT_SCHEMA.into(),
                workflow_id: "wf-1".into(),
                state: AdmissionState::Queued,
                queue_position: Some(2),
                retry_after_ms: None,
                receipt_id: "receipt-1".into(),
            })
        }

        fn progress(&self, request: &ProgressRequest) -> Result<ProgressSnapshot, HubError> {
            self.progress_calls.lock().unwrap().push(request.clone());
            Ok(ProgressSnapshot {
                workflow_id: request.workflow_id.clone(),
                next_sequence: request.after_sequence + 1,
                events: vec![ProgressEvent::Queued {
                    sequence: request.after_sequence,
                    position: 2,
                }],
                terminal: false,
            })
        }

        fn cancel(&self, request: &CancelRequest) -> Result<LifecycleReceipt, HubError> {
            Ok(LifecycleReceipt {
                schema: LOOP_HUB_CLIENT_SCHEMA.into(),
                workflow_id: request.workflow_id.clone(),
                receipt_id: "cancel-1".into(),
                state: "cancelled".into(),
            })
        }

        fn resume(&self, request: &ResumeRequest) -> Result<LifecycleReceipt, HubError> {
            Ok(LifecycleReceipt {
                schema: LOOP_HUB_CLIENT_SCHEMA.into(),
                workflow_id: request.workflow_id.clone(),
                receipt_id: request.idempotency_key.clone(),
                state: "resumed".into(),
            })
        }
    }

    fn config(session_id: &str) -> HubClientConfig {
        let mut config = HubClientConfig::new(HubMode::Required, "code", "workspace", session_id);
        config.endpoint = Some("local://hub".into());
        config
    }

    #[test]
    fn standalone_is_explicit_and_never_connects() {
        let mut config = config("standalone");
        config.mode = HubMode::Standalone;
        let calls = AtomicUsize::new(0);
        let result = LoopHubClient::connect_with_discovery(
            config,
            &|_: &HubClientConfig| unreachable!("standalone must not discover"),
            &|_: &str| {
                calls.fetch_add(1, Ordering::SeqCst);
                unreachable!("standalone must not connect")
            },
        )
        .unwrap();
        assert!(result.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clones_reuse_one_handshake_and_distinct_sessions_attach_separately() {
        HUB_SESSIONS.lock().unwrap().clear();
        let hub = Arc::new(FakeHub::default());
        let factory_hub = Arc::clone(&hub);
        let factory = move |_: &str| Ok::<Arc<dyn HubTransport>, HubError>(factory_hub.clone());
        let client = LoopHubClient::connect(config("session-1"), &factory)
            .unwrap()
            .unwrap();
        let clone = LoopHubClient::connect(config("session-1"), &factory)
            .unwrap()
            .unwrap();
        let other_session = LoopHubClient::connect(config("session-2"), &factory)
            .unwrap()
            .unwrap();
        assert_eq!(hub.handshakes.load(Ordering::SeqCst), 2);
        let mut job = clone
            .submit_interactive(InteractiveGoal::new(
                "goal",
                "turn",
                serde_json::json!({"x": 1}),
            ))
            .unwrap();
        assert_eq!(job.admission().queue_position, Some(2));
        let _ = job.poll().unwrap();
        job.cancel("user requested").unwrap();
        job.resume(Some("checkpoint-1".into())).unwrap();
        other_session
            .submit_interactive(InteractiveGoal::new("goal-2", "turn-2", Value::Null))
            .unwrap();
        let submit = hub.submits.lock().unwrap().pop().unwrap();
        assert_eq!(submit.priority, PriorityClass::Interactive);
        assert_eq!(submit.session_id, "session-2");
        assert_eq!(client.endpoint(), "local://hub");
    }

    #[test]
    fn exposed_capabilities_share_the_negotiated_session() {
        HUB_SESSIONS.lock().unwrap().clear();
        let hub = Arc::new(FakeHub::default());
        let factory_hub = Arc::clone(&hub);
        let factory = move |_: &str| Ok::<Arc<dyn HubTransport>, HubError>(factory_hub.clone());
        let client = LoopHubClient::connect(config("surface-1"), &factory)
            .unwrap()
            .unwrap();

        let runtime = client.shared_runtime_handle();
        let map = client.shared_map_handle();
        assert_eq!(runtime.service(), SharedService::Runtime);
        assert_eq!(runtime.resource().id, "r");
        assert_eq!(map.service(), SharedService::Mapper);
        assert_eq!(map.resource().id, "m");
        assert_eq!(runtime.hub_id(), "hub-1");
        assert_eq!(map.endpoint(), "local://hub");
        assert!(runtime.shares_session_with(&map));

        client
            .interactive_transport()
            .clone()
            .submit(InteractiveGoal::new("goal", "turn", Value::Null))
            .unwrap();
        let submit = hub.submits.lock().unwrap().pop().unwrap();
        assert_eq!(submit.session_id, "surface-1");
        assert_eq!(submit.priority, PriorityClass::Interactive);
        assert_eq!(hub.handshakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn service_handles_from_different_hubs_do_not_claim_shared_sessions() {
        HUB_SESSIONS.lock().unwrap().clear();
        let factory = |_: &str| Ok::<Arc<dyn HubTransport>, HubError>(Arc::new(FakeHub::default()));
        let first = LoopHubClient::connect(config("first"), &factory)
            .unwrap()
            .unwrap();
        let mut second_config = config("second");
        second_config.endpoint = Some("local://another-hub".into());
        let second = LoopHubClient::connect(second_config, &factory)
            .unwrap()
            .unwrap();

        assert!(
            !first
                .shared_runtime_handle()
                .shares_session_with(&second.shared_runtime_handle())
        );
    }

    /// Manual hot-path benchmark. Run with `--release --ignored --nocapture`;
    /// the normal suite ignores it so timing does not make CI flaky.
    #[test]
    #[ignore = "manual Loop Hub capability-clone benchmark"]
    fn benchmark_interactive_transport_clone() {
        use std::hint::black_box;
        use std::time::Instant;

        HUB_SESSIONS.lock().unwrap().clear();
        let factory = |_: &str| Ok::<Arc<dyn HubTransport>, HubError>(Arc::new(FakeHub::default()));
        let client = LoopHubClient::connect(config("benchmark"), &factory)
            .unwrap()
            .unwrap();
        let transport = client.interactive_transport();
        const ITERATIONS: u32 = 1_000_000;
        let started = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(transport.clone());
        }
        let elapsed = started.elapsed();
        eprintln!(
            "interactive transport clone: {ITERATIONS} iterations in {elapsed:?} ({:.1} ns/op)",
            elapsed.as_nanos() as f64 / f64::from(ITERATIONS)
        );
    }

    #[test]
    fn topology_rejects_duplicate_or_local_owners() {
        let mut topology = handshake();
        topology.services[0].owner = ServiceOwner::CodeProcess;
        topology.services.push(topology.services[1].clone());
        let error = topology.validate().unwrap_err().to_string();
        assert!(error.contains("runtime must be owned by Loop Hub"));
        assert!(error.contains("duplicate mapper"));
    }

    #[test]
    fn topology_rejects_missing_external_agent() {
        let mut topology = handshake();
        topology.agent.ready = false;
        topology.agent.agent_id.clear();
        let error = topology.validate().unwrap_err().to_string();
        assert!(error.contains("versioned Simplicio Agent is required"));
    }

    #[test]
    fn required_mode_fails_closed_without_endpoint() {
        let mut config = config("no-endpoint");
        config.endpoint = Some("".into());
        let result = LoopHubClient::connect_with_discovery(
            config,
            &|_: &HubClientConfig| Ok(None),
            &|_: &str| unreachable!("missing endpoint must fail before transport connection"),
        );
        assert!(matches!(result, Err(HubError::RequiredHubUnavailable)));
    }

    #[test]
    fn injected_discovery_and_handshake_use_the_external_contract() {
        HUB_SESSIONS.lock().unwrap().clear();
        let hub = Arc::new(FakeHub::default());
        let factory_hub = Arc::clone(&hub);
        let seen_endpoint = Arc::new(Mutex::new(None));
        let factory_endpoint = Arc::clone(&seen_endpoint);
        let factory = move |endpoint: &str| {
            *factory_endpoint.lock().unwrap() = Some(endpoint.to_owned());
            Ok::<Arc<dyn HubTransport>, HubError>(factory_hub.clone())
        };
        let client = LoopHubClient::connect_with_discovery(
            config_without_endpoint("discovered"),
            &|config: &HubClientConfig| {
                assert_eq!(config.client_id, "code");
                assert_eq!(config.workspace_id, "workspace");
                Ok(Some(" discovered://hub ".into()))
            },
            &factory,
        )
        .unwrap()
        .unwrap();

        assert_eq!(client.endpoint(), "discovered://hub");
        assert_eq!(
            seen_endpoint.lock().unwrap().as_deref(),
            Some("discovered://hub")
        );
        let requests = hub.handshake_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].schema, LOOP_HUB_CLIENT_SCHEMA);
        assert_eq!(requests[0].protocol, LOOP_HUB_PROTOCOL);
        assert_eq!(requests[0].client_id, "code");
        assert_eq!(requests[0].workspace_id, "workspace");
        assert_eq!(requests[0].session_id, "discovered");
    }

    #[test]
    fn explicit_endpoint_wins_over_discovery() {
        HUB_SESSIONS.lock().unwrap().clear();
        let hub = Arc::new(FakeHub::default());
        let factory_hub = Arc::clone(&hub);
        let factory = move |_: &str| Ok::<Arc<dyn HubTransport>, HubError>(factory_hub.clone());
        let mut config = config_without_endpoint("explicit");
        config.endpoint = Some(" explicit://hub ".into());
        let client = LoopHubClient::connect_with_discovery(
            config,
            &|_: &HubClientConfig| -> Result<Option<String>, HubError> {
                unreachable!("configured endpoint must take precedence")
            },
            &factory,
        )
        .unwrap()
        .unwrap();

        assert_eq!(client.endpoint(), "explicit://hub");
    }

    #[test]
    fn idempotency_keys_are_unambiguous_and_stable() {
        assert_eq!(
            idempotency_key(&["session", "turn", "goal"]),
            "7:session|4:turn|4:goal"
        );
        assert_ne!(
            idempotency_key(&["a:b", "c"]),
            idempotency_key(&["a", "b:c"])
        );
    }

    #[test]
    fn lifecycle_receipts_fail_closed_on_wrong_workflow_or_missing_evidence() {
        let receipt = LifecycleReceipt {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            workflow_id: "other-workflow".into(),
            receipt_id: String::new(),
            state: "cancelled".into(),
        };
        assert!(validate_lifecycle_receipt(&receipt, "wf-1").is_err());

        let valid = LifecycleReceipt {
            workflow_id: "wf-1".into(),
            receipt_id: "receipt-1".into(),
            ..receipt
        };
        assert!(validate_lifecycle_receipt(&valid, "wf-1").is_ok());
    }

    #[test]
    fn every_progress_variant_exposes_its_causal_sequence() {
        let events = [
            ProgressEvent::Queued {
                sequence: 1,
                position: 1,
            },
            ProgressEvent::Started {
                sequence: 2,
                worker_id: "worker".into(),
            },
            ProgressEvent::Output {
                sequence: 3,
                text: "output".into(),
            },
            ProgressEvent::Throttled {
                sequence: 4,
                retry_after_ms: 1,
            },
            ProgressEvent::Cancelled {
                sequence: 5,
                receipt_id: "r".into(),
            },
            ProgressEvent::Resumed {
                sequence: 6,
                receipt_id: "r".into(),
            },
            ProgressEvent::Completed {
                sequence: 7,
                receipt_id: "r".into(),
            },
            ProgressEvent::Failed {
                sequence: 8,
                message: "failed".into(),
                receipt_id: "r".into(),
            },
        ];
        assert_eq!(
            events.iter().map(event_sequence).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
    }

    fn config_without_endpoint(session_id: &str) -> HubClientConfig {
        let mut config = config(session_id);
        config.endpoint = None;
        config
    }
}
