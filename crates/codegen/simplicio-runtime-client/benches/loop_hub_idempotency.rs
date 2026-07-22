use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn encode(parts: &[&str]) -> String {
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

fn benchmark(c: &mut Criterion) {
    let parts = [
        "desktop-session-42",
        "turn-with:user:delimiter",
        "finish-project-issues",
    ];
    c.bench_function("loop_hub_idempotency_key", |b| {
        b.iter(|| black_box(encode(black_box(&parts))))
    });
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
