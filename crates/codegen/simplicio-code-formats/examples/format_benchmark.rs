use simplicio_code_formats::{
    HbiReader, HbiSection, HbpRecord, decode_hbp, encode_hbi, encode_hbp,
};
use std::hint::black_box;
use std::time::Instant;

fn rate(label: &str, iterations: usize, elapsed: std::time::Duration, bytes: usize) {
    let micros = elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64;
    println!("| {label} | {iterations} | {bytes} | {micros:.3} |");
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10_000);
    let sections = [HbiSection {
        kind: 1,
        bytes: vec![0x5a; 64 * 1024],
    }];
    let hbi = encode_hbi("simplicio.benchmark/v1", &sections).unwrap();
    let receipt_records: Vec<_> = (0..32)
        .map(|sequence| HbpRecord {
            sequence,
            payload: vec![sequence as u8; 128],
        })
        .collect();
    let hbp = encode_hbp(&receipt_records).unwrap();

    println!("| Operation | Iterations | Artifact bytes | Mean us/op |");
    println!("|---|---:|---:|---:|");
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(HbiReader::open(black_box(&hbi)).unwrap());
    }
    rate(
        "HBI warm validate/read (64 KiB)",
        iterations,
        started.elapsed(),
        hbi.len(),
    );

    let started = Instant::now();
    for _ in 0..iterations {
        black_box(decode_hbp(black_box(&hbp)).unwrap());
    }
    rate(
        "HBP decode (32 records)",
        iterations,
        started.elapsed(),
        hbp.len(),
    );
}
