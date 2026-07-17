//! `simplicio-bench`: run the golden-task corpus and print a JSON `BenchReport`.
//!
//! Usage:
//!   simplicio-bench [--repeats N] [--out PATH]
//!
//! Env:
//!   SIMPLICIO_BIN - optional path to a Simplicio Runtime binary. If unset (the
//!   common case, since no Runtime binary ships with or is built by this
//!   repo), the `runtime_attempt` path for every task will fail closed and be
//!   reported as such (not silently skipped, not faked as a success).

use simplicio_perf_bench::{
    BenchReport, GOLDEN_TASKS, LatencyStats, TaskKind, TaskResult, approx_tokens,
    generate_large_synthetic, resolve_fixture_path, time_repeats,
};
use std::path::PathBuf;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut repeats: usize = 20;
    let mut out_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--repeats" => {
                repeats = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(repeats);
                i += 2;
            }
            "--out" => {
                out_path = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let fixtures_root = fixtures_root();
    if repeats == 0 {
        eprintln!("--repeats must be greater than zero");
        std::process::exit(2);
    }

    // Keep the generated file inside the Runtime workspace root so the MCP
    // path-security checks exercise the actual 4MiB fixture rather than an
    // accidentally-invalid path. It is removed after the run and is never
    // committed.
    let generated_dir = fixtures_root.join(".generated");
    std::fs::create_dir_all(&generated_dir).expect("create generated fixture dir");

    // Materialize the large-synthetic fixture deterministically once, outside
    // git, so the "large file" scenario is reproducible without a committed
    // multi-megabyte blob.
    let large_bytes = generate_large_synthetic(4 * 1024 * 1024);
    std::fs::write(generated_dir.join("large_synthetic.bin"), &large_bytes)
        .expect("write synthetic large fixture");

    let mut tasks_out = Vec::new();

    for task in GOLDEN_TASKS {
        let path = resolve_fixture_path(&fixtures_root, task, &generated_dir);
        let expect_success = !matches!(task.kind, TaskKind::InvalidPath);

        // Path A: direct read, equivalent to `LocalFs::read_file` (the code
        // path `SimplicioRuntimeFs` itself wraps for writes and that the
        // Runtime read replaces for reads).
        let (samples, results): (Vec<f64>, Vec<Result<Vec<u8>, std::io::Error>>) =
            time_repeats(repeats, || std::fs::read(&path));
        let successes = results.iter().filter(|r| r.is_ok()).count();
        let success_rate = successes as f64 / results.len().max(1) as f64;
        let expected_bytes = results.iter().rev().find_map(|r| r.as_ref().ok());
        let tokens = expected_bytes.map(|bytes| approx_tokens(bytes.len()));

        tasks_out.push(TaskResult {
            id: task.id.to_string(),
            path_kind: "direct_read".to_string(),
            expected_success: expect_success,
            success_rate,
            latency: LatencyStats::from_samples(samples),
            approx_tokens: tokens,
            content_bytes: expected_bytes.map(Vec::len),
            content_matches_direct: None,
            notes: if expect_success == (success_rate > 0.0) {
                Some(
                    "UNVERIFIED| runtime capability gap: native direct_read control/fallback; not evidence of Runtime behavior."
                        .into(),
                )
            } else {
                Some("success rate did not match the expected outcome for this task".into())
            },
        });

        // Path B: attempt the real Simplicio Runtime read via the MCP client.
        // This is measured honestly: if no Runtime binary is resolvable (the
        // default in this repo, since the Runtime server is not implemented
        // here), every attempt fails closed and that is exactly what gets
        // reported.
        let repo_root = fixtures_root.clone();
        let relative = match task.kind {
            TaskKind::LargeSynthetic => PathBuf::from(".generated/large_synthetic.bin"),
            TaskKind::InvalidPath => PathBuf::from("does/not/exist.rs"),
            _ => PathBuf::from(task.fixture_relative_path.expect("fixture path set")),
        };
        let (rt_samples, rt_results): (Vec<f64>, Vec<Result<Vec<u8>, String>>) = time_repeats(
            repeats,
            || match simplicio_runtime_client::RuntimeClient::spawn_in(&repo_root) {
                Ok(mut client) => client
                    .read_file(
                        &repo_root,
                        &relative,
                        simplicio_runtime_client::DEFAULT_MAX_FILE_BYTES,
                    )
                    .and_then(|read| read.bytes())
                    .map_err(|error| format!("read: {error}")),
                Err(error) => Err(format!("spawn/initialize: {error}")),
            },
        );
        let rt_content = rt_results
            .iter()
            .rev()
            .find_map(|result| result.as_ref().ok());
        let content_matches_direct = rt_content.map(|runtime| {
            expected_bytes
                .map(|direct| runtime == direct)
                .unwrap_or(false)
        });
        // A successful MCP call is only a correctness success when its bytes
        // match the direct fixture. Invalid-path is expected to fail closed.
        let rt_successes = rt_results
            .iter()
            .filter(
                |result| match (expect_success, result.as_ref().ok(), expected_bytes) {
                    (true, Some(runtime), Some(direct)) => runtime == direct,
                    (false, None, _) => result
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.starts_with("read:")),
                    _ => false,
                },
            )
            .count();
        let rt_success_rate = rt_successes as f64 / rt_results.len().max(1) as f64;
        let rt_note = if let Some(matches) = content_matches_direct {
            format!(
                "Runtime MCP returned content; byte-for-byte comparable={matches}; approx_tokens uses the same bytes/4 heuristic."
            )
        } else if !expect_success
            && rt_results.iter().all(|result| {
                result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.starts_with("read:"))
            })
        {
            "Runtime MCP correctly rejected the invalid path; no content to compare.".to_string()
        } else {
            let error = rt_results
                .iter()
                .find_map(|result| result.as_ref().err())
                .map(String::as_str)
                .unwrap_or("unknown Runtime/MCP error");
            format!(
                "UNVERIFIED| runtime capability gap: Runtime MCP returned no readable content ({error}); native direct_read is the fallback."
            )
        };

        tasks_out.push(TaskResult {
            id: task.id.to_string(),
            path_kind: "runtime_attempt".to_string(),
            // No Runtime binary ships with (or is built by) this repo; whether this
            // succeeds depends entirely on whatever SIMPLICIO_BIN/PATH resolves to
            // on the machine running the benchmark. Do not assume either outcome.
            expected_success: expect_success,
            success_rate: rt_success_rate,
            latency: LatencyStats::from_samples(rt_samples),
            approx_tokens: rt_content.map(|bytes| approx_tokens(bytes.len())),
            content_bytes: rt_content.map(Vec::len),
            content_matches_direct,
            notes: Some(rt_note),
        });
    }

    let _ = std::fs::remove_dir_all(&generated_dir);

    let report = BenchReport {
        schema: BenchReport::SCHEMA.to_string(),
        repeats,
        tasks: tasks_out,
    };

    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = out_path {
        std::fs::write(&path, &json).expect("write report");
        eprintln!("wrote report to {}", path.display());
    }
    println!("{json}");
}
