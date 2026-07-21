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
    pub services: Vec<ServiceOwnership>,
    pub resources: SharedResources,
    pub queue: QueueCapabilities,
    /// The Hub must be the only scheduler owner. Code uses this to fail closed
    /// instead of silently creating a local fan-out loop.
    #[serde(default)]
    pub local_scheduler: bool,
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
                errors.push(format!("{} has no process identity", service_name(service.name)));
            }
            if owners.insert(service.name, service).is_some() {
                errors.push(format!("duplicate {} service declaration", service_name(service.name)));
            }
        }
        for service in [
            SharedService::Runtime,
            SharedService::Mapper,
            SharedService::Scheduler,
            SharedService::Inference,
        ] {
            if !owners.contains_key(&service) {
                errors.push(format!("missing {} service declaration", service_name(service)));
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

    fn endpoint(&self) -> Option<String> {
        self.endpoint
            .clone()
            .filter(|endpoint| !endpoint.trim().is_empty())
            .or_else(|| env::var(HUB_ENDPOINT_ENV).ok())
            .filter(|endpoint| !endpoint.trim().is_empty())
    }

    fn validate(&self) -> Result<(), HubError> {
        for (name, value) in [
            ("client_id", &self.client_id),
            ("workspace_id", &self.workspace_id),
            ("session_id", &self.session_id),
        ] {
            if value.trim().is_empty() {
                return Err(HubError::InvalidRequest(format!("{name} must not be empty")));
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("Loop Hub endpoint was not discovered; set {HUB_ENDPOINT_ENV} or configure an endpoint")]
    EndpointNotFound,
    #[error("Loop Hub is required but standalone mode was requested")]
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
    Queued { sequence: u64, position: u32 },
    Started { sequence: u64, worker_id: String },
    Output { sequence: u64, text: String },
    Throttled { sequence: u64, retry_after_ms: u64 },
    Cancelled { sequence: u64, receipt_id: String },
    Resumed { sequence: u64, receipt_id: String },
    Completed { sequence: u64, receipt_id: String },
    Failed { sequence: u64, message: String, receipt_id: String },
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

impl LoopHubClient {
    pub fn connect<F: HubTransportFactory>(
        config: HubClientConfig,
        factory: &F,
    ) -> Result<Option<Self>, HubError> {
        config.validate()?;
        if config.mode == HubMode::Standalone {
            return Ok(None);
        }
        let endpoint = config.endpoint().ok_or_else(|| {
            if config.mode == HubMode::Required {
                HubError::RequiredHubUnavailable
            } else {
                HubError::EndpointNotFound
            }
        })?;
        // One physical transport/session per Hub endpoint and workspace. The
        // logical Code session id remains on each clone so multiple surfaces
        // can multiplex over the same Hub connection without sharing turn
        // state accidentally.
        let key = format!("{}\0{}", endpoint, config.workspace_id);
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

    pub fn submit_interactive(&self, goal: InteractiveGoal) -> Result<HubJob, HubError> {
        goal.validate()?;
        let idempotency_key = format!(
            "{}:{}:{}",
            self.config.session_id, goal.turn_id, goal.goal_id
        );
        let receipt = self.session.transport.submit(&SubmitRequest {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            session_id: self.config.session_id.clone(),
            goal_id: goal.goal_id,
            turn_id: goal.turn_id,
            idempotency_key,
            priority: PriorityClass::Interactive,
            deadline_unix_ms: goal.deadline_unix_ms,
            budget_tokens: goal.budget_tokens,
            payload: goal.payload,
        })?;
        if receipt.schema != LOOP_HUB_CLIENT_SCHEMA || receipt.workflow_id.trim().is_empty() {
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
        let snapshot = self.session.transport.progress(&ProgressRequest {
            workflow_id: self.workflow_id.clone(),
            after_sequence: self.next_sequence,
        })?;
        if snapshot.workflow_id != self.workflow_id {
            return Err(HubError::Protocol(
                "Hub returned progress for another workflow".into(),
            ));
        }
        self.next_sequence = snapshot.next_sequence;
        Ok(snapshot)
    }

    pub fn cancel(&self, reason: impl Into<String>) -> Result<LifecycleReceipt, HubError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(HubError::InvalidRequest("cancel reason must not be empty".into()));
        }
        self.session.transport.cancel(&CancelRequest {
            workflow_id: self.workflow_id.clone(),
            idempotency_key: format!("{}:cancel", self.workflow_id),
            reason,
        })
    }

    pub fn resume(&mut self, checkpoint: Option<String>) -> Result<LifecycleReceipt, HubError> {
        let receipt = self.session.transport.resume(&ResumeRequest {
            workflow_id: self.workflow_id.clone(),
            idempotency_key: format!("{}:resume", self.workflow_id),
            checkpoint,
        })?;
        self.next_sequence = 0;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeHub {
        handshakes: AtomicUsize,
        submits: Mutex<Vec<SubmitRequest>>,
        progress_calls: Mutex<Vec<ProgressRequest>>,
    }

    fn handshake() -> HubHandshake {
        HubHandshake {
            schema: LOOP_HUB_CLIENT_SCHEMA.into(),
            protocol: LOOP_HUB_PROTOCOL.into(),
            hub_id: "hub-1".into(),
            ready: true,
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
                runtime: ResourceHandle { id: "r".into(), capacity: 1, used: 0 },
                mapper: ResourceHandle { id: "m".into(), capacity: 1, used: 0 },
                inference: ResourceHandle { id: "i".into(), capacity: 2, used: 0 },
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
        fn handshake(&self, _: &HubHandshakeRequest) -> Result<HubHandshake, HubError> {
            self.handshakes.fetch_add(1, Ordering::SeqCst);
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
                events: vec![ProgressEvent::Queued { sequence: request.after_sequence, position: 2 }],
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
        let result = LoopHubClient::connect(config, &|_| {
            calls.fetch_add(1, Ordering::SeqCst);
            unreachable!("standalone must not connect")
        })
        .unwrap();
        assert!(result.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clones_reuse_one_handshake_and_forward_interactive_priority() {
        HUB_SESSIONS.lock().unwrap().clear();
        let hub = Arc::new(FakeHub::default());
        let factory_hub = Arc::clone(&hub);
        let factory = move |_| Ok::<Arc<dyn HubTransport>, HubError>(factory_hub.clone());
        let client = LoopHubClient::connect(config("session-1"), &factory).unwrap().unwrap();
        let clone = LoopHubClient::connect(config("session-1"), &factory).unwrap().unwrap();
        let other_session =
            LoopHubClient::connect(config("session-2"), &factory).unwrap().unwrap();
        assert_eq!(hub.handshakes.load(Ordering::SeqCst), 1);
        let mut job = clone
            .submit_interactive(InteractiveGoal::new("goal", "turn", serde_json::json!({"x": 1})))
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
    fn topology_rejects_duplicate_or_local_owners() {
        let mut topology = handshake();
        topology.services[0].owner = ServiceOwner::CodeProcess;
        topology.services.push(topology.services[1].clone());
        let error = topology.validate().unwrap_err().to_string();
        assert!(error.contains("runtime must be owned by Loop Hub"));
        assert!(error.contains("duplicate mapper"));
    }

    #[test]
    fn required_mode_fails_closed_without_endpoint() {
        let mut config = config("no-endpoint");
        config.endpoint = Some("".into());
        let result = LoopHubClient::connect(config, &|_| unreachable!());
        assert!(matches!(result, Err(HubError::RequiredHubUnavailable)));
    }
}
