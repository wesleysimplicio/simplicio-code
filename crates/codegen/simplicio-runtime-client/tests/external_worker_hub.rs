//! Adapter-backed worker E2E against an already-running Loop Hub process.
//!
//! The Python harness owns only process lifecycle and readiness. Every worker
//! request in this test crosses `SocketWorkerHubTransport`, so the receipt is
//! evidence for the public Code adapter rather than a hand-built wire client.

use serde_json::json;
use simplicio_runtime_client::agent_workers::{
    DelegateRequest, DeliveryRequest, ExternalWorkerRun, RunIdentity, SocketWorkerHubTransport,
    WORKER_ADAPTER_SCHEMA, WORKER_PROTOCOL, WorkerHubTransport, WorkerRole, WorkerTask,
};
use std::{env, fs, path::PathBuf, sync::Arc};

fn required_env(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required for this opt-in E2E"))
}

fn request() -> DelegateRequest {
    DelegateRequest {
        schema: WORKER_ADAPTER_SCHEMA.into(),
        protocol: WORKER_PROTOCOL.into(),
        identity: RunIdentity {
            coordinator_id: "agent-host-e2e".into(),
            session_id: "session-e2e".into(),
            turn_id: "turn-e2e".into(),
            run_id: "run-e2e".into(),
            goal_id: "goal-e2e".into(),
        },
        idempotency_key: "code-worker-adapter-e2e-v1".into(),
        max_concurrency: 2,
        tasks: vec![
            WorkerTask {
                task_id: "implement".into(),
                role: WorkerRole::Implementer,
                depends_on: vec![],
                task_contract: "perform workspace effects only through Runtime".into(),
            },
            WorkerTask {
                task_id: "review".into(),
                role: WorkerRole::Reviewer,
                depends_on: vec!["implement".into()],
                task_contract: "independently review the external change".into(),
            },
        ],
    }
}

fn write_receipt(path: &str, receipt: serde_json::Value) {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create E2E receipt directory");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&receipt).expect("encode E2E receipt"),
    )
    .expect("write E2E receipt");
}

#[test]
fn worker_adapter_e2e_uses_real_loop_hub() {
    let phase = required_env("SIMPLICIO_WORKER_E2E_PHASE");
    let endpoint = required_env("SIMPLICIO_WORKER_E2E_ENDPOINT");
    let output = required_env("SIMPLICIO_WORKER_E2E_OUTPUT");
    let transport = Arc::new(
        SocketWorkerHubTransport::connect(&endpoint).expect("connect Code worker adapter"),
    );

    match phase.as_str() {
        "initial" => {
            let request = request();
            request.validate().expect("worker request validates");
            let first = transport
                .delegate(&request)
                .expect("first delegate through Rust adapter");
            let replay = transport
                .delegate(&request)
                .expect("idempotent delegate replay through Rust adapter");
            assert_eq!(first.workflow_id, replay.workflow_id);

            let mut run = ExternalWorkerRun::delegate(transport.clone(), request)
                .expect("attach reduced worker view through Rust adapter");
            assert_eq!(run.workflow_id(), first.workflow_id);
            let initial_waiting_tasks = {
                let initial = run.poll().expect("poll waiting worker events");
                assert_eq!(initial.len(), 2);
                assert!(initial.values().all(|status| {
                    status.state == simplicio_runtime_client::agent_workers::WorkerState::Waiting
                }));
                initial.len()
            };

            run.cancel("adapter E2E cancellation")
                .expect("cancel through Rust adapter");
            let cancelled_tasks = {
                let cancelled = run.poll().expect("poll cancelled worker events");
                assert_eq!(cancelled.len(), 2);
                assert!(cancelled.values().all(|status| {
                    status.state == simplicio_runtime_client::agent_workers::WorkerState::Cancelled
                }));
                cancelled.len()
            };

            let blocked = transport.deliver(&DeliveryRequest {
                workflow_id: first.workflow_id.clone(),
                task_id: "review".into(),
                agent_id: "external-agent:review".into(),
                attempt: 1,
                fence: 1,
                review_receipt_id: "review-e2e".into(),
                idempotency_key: "delivery-e2e".into(),
            });
            assert!(blocked.is_err(), "cancelled delivery must fail closed");

            write_receipt(
                &output,
                json!({
                    "schema": "simplicio.code-worker-adapter-e2e/v1",
                    "proof_kind": "rust_adapter_external_loop_hub_process",
                    "phase": "initial",
                    "workflow_id": first.workflow_id,
                    "idempotent_delegate": true,
                    "initial_waiting_tasks": initial_waiting_tasks,
                    "cancelled_tasks": cancelled_tasks,
                    "delivery_blocked": true,
                    "local_llm_started": false,
                    "deepseek_started": false,
                }),
            );
        }
        "restart" => {
            let workflow_id = required_env("SIMPLICIO_WORKER_E2E_WORKFLOW");
            let mut run = ExternalWorkerRun::attach(transport, workflow_id.clone())
                .expect("reattach reduced worker view after Hub restart");
            let statuses = run.poll().expect("replay durable worker events");
            assert_eq!(statuses.len(), 2);
            assert!(statuses.values().all(|status| {
                status.state == simplicio_runtime_client::agent_workers::WorkerState::Cancelled
            }));
            write_receipt(
                &output,
                json!({
                    "schema": "simplicio.code-worker-adapter-e2e/v1",
                    "proof_kind": "rust_adapter_external_loop_hub_process",
                    "phase": "restart",
                    "workflow_id": workflow_id,
                    "restart_persisted": true,
                    "replayed_cancelled_tasks": statuses.len(),
                    "local_llm_started": false,
                    "deepseek_started": false,
                }),
            );
        }
        other => panic!("unsupported worker E2E phase: {other}"),
    }
}
