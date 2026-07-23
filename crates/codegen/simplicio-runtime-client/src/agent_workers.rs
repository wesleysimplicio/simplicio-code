//! Transport-only adapter for externally materialized Loop workers.
//!
//! This module deliberately does not create worktrees, run commands, schedule
//! waves, or select a model.  Code submits a validated DAG to the already
//! running Loop Hub and reduces the Hub's causal receipts into UI state.  The
//! Hub remains the scheduler, AgentHost remains the reasoning authority, and
//! Runtime remains the only workspace/process authority.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const WORKER_ADAPTER_SCHEMA: &str = "simplicio.code-worker-adapter/v1";
pub const WORKER_PROTOCOL: &str = "simplicio.loop-worker/v1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkerError {
    #[error("invalid worker request: {0}")]
    Invalid(String),
    #[error("worker authority rejected: {0}")]
    Authority(String),
    #[error("Loop Hub worker protocol error: {0}")]
    Protocol(String),
    #[error("Loop Hub worker transport unavailable: {0}")]
    Transport(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunIdentity {
    pub coordinator_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub run_id: String,
    pub goal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptIdentity {
    pub stage_id: String,
    pub agent_id: String,
    pub worktree_id: String,
    pub attempt: u32,
    pub fence: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Implementer,
    Reviewer,
    Tester,
    Delivery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerTask {
    pub task_id: String,
    pub role: WorkerRole,
    pub depends_on: Vec<String>,
    /// Opaque task contract for the external Agent. Code never interprets it
    /// as an instruction to an embedded or local provider.
    pub task_contract: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DelegateRequest {
    pub schema: String,
    pub protocol: String,
    pub identity: RunIdentity,
    pub idempotency_key: String,
    pub max_concurrency: u32,
    pub tasks: Vec<WorkerTask>,
}

impl DelegateRequest {
    pub fn new(identity: RunIdentity, max_concurrency: u32, tasks: Vec<WorkerTask>) -> Self {
        let idempotency_key = framed_key(&[
            &identity.session_id,
            &identity.turn_id,
            &identity.run_id,
            &identity.goal_id,
        ]);
        Self {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            protocol: WORKER_PROTOCOL.into(),
            identity,
            idempotency_key,
            max_concurrency,
            tasks,
        }
    }

    pub fn validate(&self) -> Result<(), WorkerError> {
        if self.schema != WORKER_ADAPTER_SCHEMA || self.protocol != WORKER_PROTOCOL {
            return Err(WorkerError::Invalid(
                "unsupported schema or protocol".into(),
            ));
        }
        for (name, value) in [
            ("coordinator_id", &self.identity.coordinator_id),
            ("session_id", &self.identity.session_id),
            ("turn_id", &self.identity.turn_id),
            ("run_id", &self.identity.run_id),
            ("goal_id", &self.identity.goal_id),
        ] {
            if value.trim().is_empty() {
                return Err(WorkerError::Invalid(format!("{name} must not be empty")));
            }
        }
        if self.max_concurrency == 0 || self.tasks.is_empty() {
            return Err(WorkerError::Invalid(
                "tasks and bounded max_concurrency are required".into(),
            ));
        }
        let ids = self
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<BTreeSet<_>>();
        if ids.len() != self.tasks.len()
            || self
                .tasks
                .iter()
                .any(|task| task.task_id.trim().is_empty() || task.task_contract.trim().is_empty())
        {
            return Err(WorkerError::Invalid(
                "task IDs/contracts must be non-empty and IDs unique".into(),
            ));
        }
        for task in &self.tasks {
            if task
                .depends_on
                .iter()
                .any(|dependency| dependency == &task.task_id || !ids.contains(dependency.as_str()))
            {
                return Err(WorkerError::Invalid(format!(
                    "task {} has a missing or self dependency",
                    task.task_id
                )));
            }
        }
        detect_cycle(&self.tasks)?;
        Ok(())
    }
}

fn detect_cycle(tasks: &[WorkerTask]) -> Result<(), WorkerError> {
    fn visit<'a>(
        id: &'a str,
        graph: &BTreeMap<&'a str, &'a WorkerTask>,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> Result<(), WorkerError> {
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id) {
            return Err(WorkerError::Invalid("task DAG contains a cycle".into()));
        }
        for dependency in &graph[id].depends_on {
            visit(dependency, graph, visiting, visited)?;
        }
        visiting.remove(id);
        visited.insert(id);
        Ok(())
    }
    let graph = tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in graph.keys() {
        visit(id, &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DelegateReceipt {
    pub schema: String,
    pub workflow_id: String,
    pub receipt_id: String,
    pub accepted_task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStatusRequest {
    pub workflow_id: String,
    pub after_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStatusReceipt {
    pub schema: String,
    pub workflow_id: String,
    pub next_sequence: u64,
    pub events: Vec<WorkerEvent>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Waiting,
    Working,
    Blocked,
    Failed,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeLease {
    pub worktree_id: String,
    pub branch: String,
    pub path_token: String,
    pub lease_id: String,
    pub fence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerEvent {
    pub sequence: u64,
    pub event_id: String,
    pub causal_event_id: Option<String>,
    pub task_id: String,
    pub role: WorkerRole,
    pub attempt: AttemptIdentity,
    pub state: WorkerState,
    pub owner: String,
    pub reason: Option<String>,
    pub lease: Option<WorktreeLease>,
    pub receipt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub task_id: String,
    pub role: WorkerRole,
    pub state: WorkerState,
    pub owner: String,
    pub last_event_id: String,
    pub reason: Option<String>,
    pub attempt: AttemptIdentity,
    pub lease: Option<WorktreeLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelWorkersRequest {
    pub workflow_id: String,
    pub idempotency_key: String,
    pub reason: String,
    pub revoke_mutation_authority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryRequest {
    pub workflow_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub attempt: u32,
    pub fence: u64,
    pub review_receipt_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pub schema: String,
    pub workflow_id: String,
    pub receipt_id: String,
    pub remote_reference: String,
    pub remotely_confirmed: bool,
}

pub trait WorkerHubTransport: Send + Sync {
    fn delegate(&self, request: &DelegateRequest) -> Result<DelegateReceipt, WorkerError>;
    fn status(&self, request: &WorkerStatusRequest) -> Result<WorkerStatusReceipt, WorkerError>;
    fn cancel(&self, request: &CancelWorkersRequest) -> Result<DelegateReceipt, WorkerError>;
    fn deliver(&self, request: &DeliveryRequest) -> Result<DeliveryReceipt, WorkerError>;
}

/// A view of an external Hub workflow. It contains only replay cursors and
/// reduced status; it is not a scheduler and cannot execute a task locally.
pub struct ExternalWorkerRun {
    transport: Arc<dyn WorkerHubTransport>,
    workflow_id: String,
    next_sequence: u64,
    statuses: BTreeMap<String, AgentStatus>,
}

impl ExternalWorkerRun {
    pub fn delegate(
        transport: Arc<dyn WorkerHubTransport>,
        request: DelegateRequest,
    ) -> Result<Self, WorkerError> {
        request.validate()?;
        let expected = request
            .tasks
            .iter()
            .map(|task| task.task_id.as_str())
            .collect::<BTreeSet<_>>();
        let receipt = transport.delegate(&request)?;
        let accepted = receipt
            .accepted_task_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if receipt.schema != WORKER_ADAPTER_SCHEMA
            || receipt.workflow_id.trim().is_empty()
            || receipt.receipt_id.trim().is_empty()
            || accepted != expected
        {
            return Err(WorkerError::Protocol(
                "Hub returned an invalid or partial delegate receipt".into(),
            ));
        }
        Ok(Self {
            transport,
            workflow_id: receipt.workflow_id,
            next_sequence: 0,
            statuses: BTreeMap::new(),
        })
    }

    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    pub fn statuses(&self) -> &BTreeMap<String, AgentStatus> {
        &self.statuses
    }

    pub fn poll(&mut self) -> Result<&BTreeMap<String, AgentStatus>, WorkerError> {
        let receipt = self.transport.status(&WorkerStatusRequest {
            workflow_id: self.workflow_id.clone(),
            after_sequence: self.next_sequence,
        })?;
        if receipt.schema != WORKER_ADAPTER_SCHEMA
            || receipt.workflow_id != self.workflow_id
            || receipt.next_sequence < self.next_sequence
        {
            return Err(WorkerError::Protocol(
                "stale or cross-run status receipt".into(),
            ));
        }
        let mut previous_sequence = self.next_sequence.checked_sub(1);
        let mut seen = BTreeSet::new();
        for event in receipt.events {
            if event.sequence < self.next_sequence
                || event.sequence >= receipt.next_sequence
                || previous_sequence.is_some_and(|sequence| event.sequence <= sequence)
                || event.event_id.trim().is_empty()
                || !seen.insert(event.event_id.clone())
            {
                return Err(WorkerError::Protocol(
                    "non-monotonic or duplicate worker event".into(),
                ));
            }
            previous_sequence = Some(event.sequence);
            self.apply_event(event)?;
        }
        self.next_sequence = receipt.next_sequence;
        Ok(&self.statuses)
    }

    fn apply_event(&mut self, event: WorkerEvent) -> Result<(), WorkerError> {
        if let Some(current) = self.statuses.get(&event.task_id) {
            if event.attempt.attempt < current.attempt.attempt
                || (event.attempt.attempt == current.attempt.attempt
                    && event.attempt.fence < current.attempt.fence)
            {
                return Err(WorkerError::Authority(format!(
                    "late worker event for task {} lost its fence",
                    event.task_id
                )));
            }
            if event.attempt.attempt == current.attempt.attempt
                && event.attempt.fence == current.attempt.fence
                && !valid_transition(current.state, event.state)
            {
                return Err(WorkerError::Protocol(format!(
                    "invalid {:?} -> {:?} transition for {}",
                    current.state, event.state, event.task_id
                )));
            }
        }
        if matches!(event.state, WorkerState::Done)
            && event.receipt_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(WorkerError::Protocol(
                "done requires a causal receipt".into(),
            ));
        }
        if let Some(lease) = &event.lease {
            if lease.fence != event.attempt.fence
                || lease.worktree_id != event.attempt.worktree_id
                || lease.branch.trim().is_empty()
                || lease.path_token.trim().is_empty()
                || lease.lease_id.trim().is_empty()
            {
                return Err(WorkerError::Protocol(
                    "invalid worktree lease identity".into(),
                ));
            }
            if self.statuses.values().any(|status| {
                status.task_id != event.task_id
                    && status.lease.as_ref().is_some_and(|other| {
                        other.worktree_id == lease.worktree_id
                            || other.branch == lease.branch
                            || other.path_token == lease.path_token
                    })
                    && !matches!(status.state, WorkerState::Cancelled | WorkerState::Failed)
            }) {
                return Err(WorkerError::Authority(
                    "worktree/branch collision between active workers".into(),
                ));
            }
        }
        self.statuses.insert(
            event.task_id.clone(),
            AgentStatus {
                task_id: event.task_id,
                role: event.role,
                state: event.state,
                owner: event.owner,
                last_event_id: event.event_id,
                reason: event.reason,
                attempt: event.attempt,
                lease: event.lease,
            },
        );
        Ok(())
    }

    pub fn cancel(&self, reason: impl Into<String>) -> Result<DelegateReceipt, WorkerError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(WorkerError::Invalid("cancel reason is required".into()));
        }
        self.transport.cancel(&CancelWorkersRequest {
            workflow_id: self.workflow_id.clone(),
            idempotency_key: framed_key(&[&self.workflow_id, "cancel", &reason]),
            reason,
            revoke_mutation_authority: true,
        })
    }

    pub fn deliver(
        &self,
        task_id: &str,
        independent_review_receipt: &str,
    ) -> Result<DeliveryReceipt, WorkerError> {
        let status = self
            .statuses
            .get(task_id)
            .ok_or_else(|| WorkerError::Authority("delivery agent has no observed state".into()))?;
        if status.role != WorkerRole::Delivery || status.state != WorkerState::Done {
            return Err(WorkerError::Authority(
                "only a done delivery-role agent may request delivery".into(),
            ));
        }
        if independent_review_receipt.trim().is_empty() {
            return Err(WorkerError::Authority(
                "independent review receipt is required".into(),
            ));
        }
        let request = DeliveryRequest {
            workflow_id: self.workflow_id.clone(),
            task_id: task_id.into(),
            agent_id: status.attempt.agent_id.clone(),
            attempt: status.attempt.attempt,
            fence: status.attempt.fence,
            review_receipt_id: independent_review_receipt.into(),
            idempotency_key: framed_key(&[
                &self.workflow_id,
                task_id,
                &status.attempt.agent_id,
                &status.attempt.attempt.to_string(),
                &status.attempt.fence.to_string(),
            ]),
        };
        let receipt = self.transport.deliver(&request)?;
        if receipt.schema != WORKER_ADAPTER_SCHEMA
            || receipt.workflow_id != self.workflow_id
            || receipt.receipt_id.trim().is_empty()
            || receipt.remote_reference.trim().is_empty()
            || !receipt.remotely_confirmed
        {
            return Err(WorkerError::Protocol(
                "delivery is not remotely confirmed".into(),
            ));
        }
        Ok(receipt)
    }
}

fn valid_transition(from: WorkerState, to: WorkerState) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                WorkerState::Waiting,
                WorkerState::Working | WorkerState::Blocked | WorkerState::Cancelled
            ) | (
                WorkerState::Working,
                WorkerState::Blocked
                    | WorkerState::Failed
                    | WorkerState::Done
                    | WorkerState::Cancelled
            ) | (
                WorkerState::Blocked,
                WorkerState::Waiting
                    | WorkerState::Working
                    | WorkerState::Failed
                    | WorkerState::Cancelled
            )
        )
}

fn framed_key(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHub {
        status: Mutex<Option<WorkerStatusReceipt>>,
        delivery: Mutex<Option<DeliveryReceipt>>,
        cancellations: Mutex<Vec<CancelWorkersRequest>>,
    }

    impl WorkerHubTransport for FakeHub {
        fn delegate(&self, request: &DelegateRequest) -> Result<DelegateReceipt, WorkerError> {
            Ok(DelegateReceipt {
                schema: WORKER_ADAPTER_SCHEMA.into(),
                workflow_id: "workflow-1".into(),
                receipt_id: "delegate-1".into(),
                accepted_task_ids: request
                    .tasks
                    .iter()
                    .map(|task| task.task_id.clone())
                    .collect(),
            })
        }
        fn status(&self, _: &WorkerStatusRequest) -> Result<WorkerStatusReceipt, WorkerError> {
            self.status
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| WorkerError::Transport("no fixture".into()))
        }
        fn cancel(&self, request: &CancelWorkersRequest) -> Result<DelegateReceipt, WorkerError> {
            self.cancellations.lock().unwrap().push(request.clone());
            Ok(DelegateReceipt {
                schema: WORKER_ADAPTER_SCHEMA.into(),
                workflow_id: request.workflow_id.clone(),
                receipt_id: "cancel-1".into(),
                accepted_task_ids: vec![],
            })
        }
        fn deliver(&self, _: &DeliveryRequest) -> Result<DeliveryReceipt, WorkerError> {
            self.delivery
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| WorkerError::Transport("no fixture".into()))
        }
    }

    fn request(tasks: Vec<WorkerTask>) -> DelegateRequest {
        DelegateRequest::new(
            RunIdentity {
                coordinator_id: "agent-host".into(),
                session_id: "s".into(),
                turn_id: "t".into(),
                run_id: "r".into(),
                goal_id: "g".into(),
            },
            2,
            tasks,
        )
    }

    fn task(id: &str, role: WorkerRole, dependencies: &[&str]) -> WorkerTask {
        WorkerTask {
            task_id: id.into(),
            role,
            depends_on: dependencies.iter().map(|value| (*value).into()).collect(),
            task_contract: format!("contract-{id}"),
        }
    }

    fn event(
        sequence: u64,
        task_id: &str,
        role: WorkerRole,
        state: WorkerState,
        attempt: u32,
        fence: u64,
        worktree: &str,
    ) -> WorkerEvent {
        WorkerEvent {
            sequence,
            event_id: format!("event-{sequence}"),
            causal_event_id: None,
            task_id: task_id.into(),
            role,
            attempt: AttemptIdentity {
                stage_id: "stage".into(),
                agent_id: format!("agent-{task_id}"),
                worktree_id: worktree.into(),
                attempt,
                fence,
            },
            state,
            owner: "loop-hub".into(),
            reason: None,
            lease: Some(WorktreeLease {
                worktree_id: worktree.into(),
                branch: format!("branch-{worktree}"),
                path_token: format!("path-{worktree}"),
                lease_id: format!("lease-{worktree}"),
                fence,
            }),
            receipt_id: matches!(state, WorkerState::Done).then(|| format!("done-{task_id}")),
        }
    }

    #[test]
    fn dag_accepts_parallel_tasks_and_dependency() {
        request(vec![
            task("a", WorkerRole::Implementer, &[]),
            task("b", WorkerRole::Implementer, &[]),
            task("review", WorkerRole::Reviewer, &["a", "b"]),
        ])
        .validate()
        .unwrap();
    }

    #[test]
    fn dag_rejects_cycles_and_unknown_dependencies() {
        assert!(request(vec![
            task("a", WorkerRole::Implementer, &["b"]),
            task("b", WorkerRole::Reviewer, &["a"])
        ])
        .validate()
        .unwrap_err()
        .to_string()
        .contains("cycle"));
        assert!(
            request(vec![task("a", WorkerRole::Implementer, &["missing"])])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn reducer_tracks_two_isolated_workers_and_rejects_collision() {
        let hub = Arc::new(FakeHub::default());
        let mut run = ExternalWorkerRun::delegate(
            hub.clone(),
            request(vec![
                task("a", WorkerRole::Implementer, &[]),
                task("b", WorkerRole::Implementer, &[]),
            ]),
        )
        .unwrap();
        *hub.status.lock().unwrap() = Some(WorkerStatusReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            next_sequence: 2,
            events: vec![
                event(
                    0,
                    "a",
                    WorkerRole::Implementer,
                    WorkerState::Working,
                    1,
                    10,
                    "wt-a",
                ),
                event(
                    1,
                    "b",
                    WorkerRole::Implementer,
                    WorkerState::Working,
                    1,
                    20,
                    "wt-b",
                ),
            ],
        });
        assert_eq!(run.poll().unwrap().len(), 2);
        *hub.status.lock().unwrap() = Some(WorkerStatusReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            next_sequence: 3,
            events: vec![event(
                2,
                "b",
                WorkerRole::Implementer,
                WorkerState::Working,
                2,
                30,
                "wt-a",
            )],
        });
        assert!(
            matches!(run.poll(), Err(WorkerError::Authority(message)) if message.contains("collision"))
        );
    }

    #[test]
    fn late_worker_loses_fence_and_done_requires_receipt() {
        let hub = Arc::new(FakeHub::default());
        let mut run = ExternalWorkerRun::delegate(
            hub.clone(),
            request(vec![task("a", WorkerRole::Implementer, &[])]),
        )
        .unwrap();
        *hub.status.lock().unwrap() = Some(WorkerStatusReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            next_sequence: 1,
            events: vec![event(
                0,
                "a",
                WorkerRole::Implementer,
                WorkerState::Working,
                2,
                20,
                "wt-a",
            )],
        });
        run.poll().unwrap();
        *hub.status.lock().unwrap() = Some(WorkerStatusReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            next_sequence: 2,
            events: vec![event(
                1,
                "a",
                WorkerRole::Implementer,
                WorkerState::Done,
                1,
                10,
                "wt-a",
            )],
        });
        assert!(matches!(run.poll(), Err(WorkerError::Authority(_))));
    }

    #[test]
    fn cancel_always_revokes_mutation_authority() {
        let hub = Arc::new(FakeHub::default());
        let run = ExternalWorkerRun::delegate(
            hub.clone(),
            request(vec![task("a", WorkerRole::Implementer, &[])]),
        )
        .unwrap();
        run.cancel("STOP").unwrap();
        assert!(hub.cancellations.lock().unwrap()[0].revoke_mutation_authority);
    }

    #[test]
    fn implementer_cannot_deliver_and_remote_confirmation_is_required() {
        let hub = Arc::new(FakeHub::default());
        let mut run = ExternalWorkerRun::delegate(
            hub.clone(),
            request(vec![
                task("build", WorkerRole::Implementer, &[]),
                task("delivery", WorkerRole::Delivery, &["build"]),
            ]),
        )
        .unwrap();
        *hub.status.lock().unwrap() = Some(WorkerStatusReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            next_sequence: 2,
            events: vec![
                event(
                    0,
                    "build",
                    WorkerRole::Implementer,
                    WorkerState::Done,
                    1,
                    10,
                    "wt-build",
                ),
                event(
                    1,
                    "delivery",
                    WorkerRole::Delivery,
                    WorkerState::Done,
                    1,
                    20,
                    "wt-delivery",
                ),
            ],
        });
        run.poll().unwrap();
        assert!(matches!(
            run.deliver("build", "review-1"),
            Err(WorkerError::Authority(_))
        ));
        *hub.delivery.lock().unwrap() = Some(DeliveryReceipt {
            schema: WORKER_ADAPTER_SCHEMA.into(),
            workflow_id: "workflow-1".into(),
            receipt_id: "delivery-1".into(),
            remote_reference: "pr/1".into(),
            remotely_confirmed: false,
        });
        assert!(matches!(
            run.deliver("delivery", "review-1"),
            Err(WorkerError::Protocol(_))
        ));
    }

    #[test]
    fn idempotency_key_is_unambiguous() {
        assert_ne!(framed_key(&["a:b", "c"]), framed_key(&["a", "b:c"]));
    }
}
