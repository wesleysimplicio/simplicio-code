//! `simplicio-bench-check`: compare a freshly produced `BenchReport` JSON file
//! against a committed baseline and exit non-zero if any tracked metric
//! regresses by more than `REGRESSION_THRESHOLD_PCT`.
//!
//! Usage:
//!   simplicio-bench-check --baseline PATH --current PATH

use simplicio_perf_bench::{BenchReport, find_regressions};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut baseline_path: Option<PathBuf> = None;
    let mut current_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--baseline" => {
                baseline_path = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--current" => {
                current_path = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let baseline_path = baseline_path.unwrap_or_else(|| {
        eprintln!("--baseline PATH is required");
        std::process::exit(2);
    });
    let current_path = current_path.unwrap_or_else(|| {
        eprintln!("--current PATH is required");
        std::process::exit(2);
    });

    let baseline: BenchReport = serde_json::from_str(
        &std::fs::read_to_string(&baseline_path).unwrap_or_else(|e| {
            eprintln!("failed to read baseline {}: {e}", baseline_path.display());
            std::process::exit(2);
        }),
    )
    .unwrap_or_else(|e| {
        eprintln!("failed to parse baseline {}: {e}", baseline_path.display());
        std::process::exit(2);
    });
    let current: BenchReport =
        serde_json::from_str(&std::fs::read_to_string(&current_path).unwrap_or_else(|e| {
            eprintln!("failed to read current {}: {e}", current_path.display());
            std::process::exit(2);
        }))
        .unwrap_or_else(|e| {
            eprintln!("failed to parse current {}: {e}", current_path.display());
            std::process::exit(2);
        });

    let regressions = find_regressions(&baseline, &current);
    if regressions.is_empty() {
        println!("no regressions vs baseline {}", baseline_path.display());
        return;
    }

    eprintln!(
        "regressions detected vs baseline {}:",
        baseline_path.display()
    );
    for r in &regressions {
        eprintln!(
            "  {} / {}: {:.4} -> {:.4} ({:+.1}%)",
            r.task_id, r.metric, r.baseline, r.current, r.pct_change
        );
    }
    std::process::exit(1);
}
