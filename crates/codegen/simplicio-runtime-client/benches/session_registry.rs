use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use simplicio_runtime_client::session_registry::{
    SessionKind, SessionMetadata, SessionQuery, SessionRegistry,
};
use std::hint::black_box;

fn metadata(project: usize) -> SessionMetadata {
    SessionMetadata {
        space_id: "space".into(),
        project_id: format!("project-{project}"),
        workspace_id: format!("workspace-{project}"),
        agent_label: "external-agent".into(),
        goal_label: format!("goal-{project}"),
        branch_label: Some("feature-session-registry".into()),
        worktree_label: Some("shared-worktree".into()),
    }
}

fn populated(count: usize) -> SessionRegistry {
    let mut registry = SessionRegistry::new();
    for index in 0..count {
        registry
            .create(
                format!("session-{index}"),
                SessionKind::Work,
                metadata(index % 10),
                format!("handle-{index}"),
                "host-a".into(),
                index as u64,
            )
            .unwrap();
    }
    registry
}

fn session_registry_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("session_registry");
    group.bench_function("cold_create", |b| {
        b.iter(|| {
            let mut registry = SessionRegistry::new();
            registry
                .create(
                    "session".into(),
                    SessionKind::Coordinator,
                    metadata(0),
                    "handle".into(),
                    "host-a".into(),
                    1,
                )
                .unwrap();
            black_box(registry)
        })
    });
    for count in [1_usize, 100] {
        let registry = populated(count);
        group.bench_with_input(BenchmarkId::new("warm_list", count), &count, |b, _| {
            b.iter(|| black_box(registry.list(&SessionQuery::default())))
        });
        group.bench_with_input(BenchmarkId::new("detail", count), &count, |b, _| {
            b.iter(|| black_box(registry.get("session-0").unwrap()))
        });
    }
    let mut registry = populated(100);
    group.bench_function("attach_detach", |b| {
        b.iter(|| {
            registry.attach("session-0", "tui", 101).unwrap();
            registry.detach("session-0", "tui", 102).unwrap();
        })
    });
    group.bench_function("replay", |b| {
        b.iter(|| black_box(registry.reconnect("session-1", "host-a", 0, 100).unwrap()))
    });
    group.bench_function("snapshot_after_100_session_churn", |b| {
        b.iter(|| black_box(serde_json::to_vec(&registry.snapshot()).unwrap()))
    });
    group.finish();
}

criterion_group!(benches, session_registry_benchmark);
criterion_main!(benches);
