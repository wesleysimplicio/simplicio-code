//! Real Code client proof against an already-running Loop Hub daemon.

use serde_json::json;
use simplicio_runtime_client::SocketPipeHubTransportFactory;
use simplicio_runtime_client::loop_hub::{HubMode, InteractiveGoal, LoopHubClient};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn exercise_external_restart(job: &mut simplicio_runtime_client::loop_hub::HubJob) -> Option<u128> {
    let ready = std::env::var_os("SIMPLICIO_LOOP_HUB_RESTART_READY").map(PathBuf::from)?;
    let complete = std::env::var_os("SIMPLICIO_LOOP_HUB_RESTART_COMPLETE").map(PathBuf::from)?;
    std::fs::write(&ready, job.workflow_id()).expect("write restart rendezvous marker");
    let started = Instant::now();
    while !complete.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "external harness did not restart Loop Hub"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let reconnect_started = Instant::now();
    let snapshot = job
        .poll()
        .expect("safe progress read should reconnect and replay after Hub restart");
    assert_eq!(snapshot.workflow_id, job.workflow_id());
    Some(reconnect_started.elapsed().as_millis())
}

#[test]
fn code_attaches_to_real_loop_hub_and_reuses_shared_services() {
    let Ok(endpoint) = std::env::var("SIMPLICIO_LOOP_HUB_ENDPOINT") else {
        // This is an opt-in external system test; normal package tests remain
        // hermetic and use the in-process transport fakes.
        return;
    };
    let surfaces = [
        ("tui-1", "session-tui-1"),
        ("tui-2", "session-tui-2"),
        ("headless", "session-headless"),
        ("acp", "session-acp"),
    ];
    let mut clients = Vec::new();
    for (surface, session) in surfaces {
        let mut config = simplicio_runtime_client::loop_hub::HubClientConfig::new(
            HubMode::Required,
            format!("code-{surface}"),
            "workspace-e2e",
            session,
        );
        config.endpoint = Some(endpoint.clone());
        let client = LoopHubClient::connect(config, &SocketPipeHubTransportFactory)
            .expect("real Hub connection should succeed")
            .expect("required mode must attach");
        assert!(client.handshake().hub_id.starts_with("loop-hub:"));
        assert_eq!(client.handshake().services.len(), 4);
        assert!(
            client
                .handshake()
                .services
                .iter()
                .all(|service| service.owner
                    == simplicio_runtime_client::loop_hub::ServiceOwner::LoopHub)
        );
        clients.push((surface, client));
    }
    let hub_id = clients[0].1.handshake().hub_id.clone();
    let runtime_id = clients[0].1.shared_runtime_handle().resource().id.clone();
    let mapper_id = clients[0].1.shared_map_handle().resource().id.clone();
    assert!(
        clients
            .iter()
            .all(|(_, client)| client.handshake().hub_id == hub_id)
    );
    assert!(
        clients
            .iter()
            .all(|(_, client)| client.shared_runtime_handle().resource().id == runtime_id)
    );
    assert!(
        clients
            .iter()
            .all(|(_, client)| client.shared_map_handle().resource().id == mapper_id)
    );

    let mut replay_job = clients[0]
        .1
        .submit_interactive(InteractiveGoal::new(
            "replay-goal",
            "replay-turn",
            json!({"provider": "none", "llm": "disabled"}),
        ))
        .expect("first idempotent submit should succeed");
    let replay_again = clients[0]
        .1
        .submit_interactive(InteractiveGoal::new(
            "replay-goal",
            "replay-turn",
            json!({"provider": "none", "llm": "disabled"}),
        ))
        .expect("replayed idempotent submit should succeed");
    assert_eq!(replay_job.workflow_id(), replay_again.workflow_id());
    assert!(
        !replay_job
            .poll()
            .expect("replay progress should succeed")
            .events
            .is_empty()
    );
    let cancelled = replay_job
        .cancel("external E2E cleanup")
        .expect("cancel should succeed");
    assert_eq!(cancelled.state, "cancelled");
    assert_eq!(
        replay_job
            .resume(None)
            .expect("resume should succeed")
            .state,
        "queued"
    );
    for (surface, client) in &clients[1..] {
        let mut job = client
            .submit_interactive(InteractiveGoal::new(
                format!("{surface}-goal"),
                format!("{surface}-turn"),
                json!({"provider": "none", "llm": "disabled", "surface": surface}),
            ))
            .expect("surface submit should succeed");
        assert!(
            !job.poll()
                .expect("surface progress should succeed")
                .events
                .is_empty()
        );
        assert_eq!(
            job.cancel("surface cleanup")
                .expect("surface cancel should succeed")
                .state,
            "cancelled"
        );
    }
    let mut restart_job = clients[0]
        .1
        .submit_interactive(InteractiveGoal::new(
            "restart-goal",
            "restart-turn",
            json!({"provider": "none", "llm": "disabled"}),
        ))
        .expect("restart probe submit should succeed");
    assert!(
        !restart_job
            .poll()
            .expect("restart probe progress should succeed")
            .events
            .is_empty()
    );
    let reconnect_ms = exercise_external_restart(&mut restart_job);

    println!(
        "hub_id={} runtime_id={} mapper_id={} surfaces=4 tui_sessions=2 headless=1 acp=1 replay=true resume=true cancelled=true restart_reconnected={} reconnect_ms={} single_hub_identity=true",
        hub_id,
        runtime_id,
        mapper_id,
        reconnect_ms.is_some(),
        reconnect_ms.map_or_else(|| "null".into(), |value| value.to_string())
    );
}
