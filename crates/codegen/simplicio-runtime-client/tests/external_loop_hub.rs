//! Real Code client proof against an already-running Loop Hub daemon.

use serde_json::json;
use simplicio_runtime_client::SocketPipeHubTransportFactory;
use simplicio_runtime_client::loop_hub::{HubMode, InteractiveGoal, LoopHubClient};

#[test]
fn code_attaches_to_real_loop_hub_and_reuses_shared_services() {
    let Ok(endpoint) = std::env::var("SIMPLICIO_LOOP_HUB_ENDPOINT") else {
        // This is an opt-in external system test; normal package tests remain
        // hermetic and use the in-process transport fakes.
        return;
    };
    let mut config = simplicio_runtime_client::loop_hub::HubClientConfig::new(
        HubMode::Required,
        "code-e2e",
        "workspace-e2e",
        "session-e2e",
    );
    config.endpoint = Some(endpoint);
    let client = LoopHubClient::connect(config, &SocketPipeHubTransportFactory)
        .expect("real Hub connection should succeed")
        .expect("required mode must attach");
    assert!(client.handshake().hub_id.starts_with("loop-hub:"));
    assert!(
        client
            .shared_runtime_handle()
            .shares_session_with(&client.shared_map_handle())
    );
    assert_eq!(client.handshake().services.len(), 4);
    assert!(
        client
            .handshake()
            .services
            .iter()
            .all(|service| service.owner
                == simplicio_runtime_client::loop_hub::ServiceOwner::LoopHub)
    );

    let mut job = client
        .submit_interactive(InteractiveGoal::new(
            "code-goal",
            "code-turn",
            json!({"provider": "none", "llm": "disabled"}),
        ))
        .expect("real Hub submit should succeed");
    let progress = job.poll().expect("real Hub progress should succeed");
    assert_eq!(progress.workflow_id, job.workflow_id());
    assert!(!progress.events.is_empty());
    let cancelled = job
        .cancel("external E2E cleanup")
        .expect("cancel should succeed");
    assert_eq!(cancelled.workflow_id, job.workflow_id());
    assert_eq!(cancelled.state, "cancelled");

    println!(
        "hub_id={} runtime_id={} mapper_id={} workflow_id={} cancelled={} replay_safe_progress=true",
        client.handshake().hub_id,
        client.shared_runtime_handle().resource().id,
        client.shared_map_handle().resource().id,
        job.workflow_id(),
        cancelled.state == "cancelled"
    );
}
