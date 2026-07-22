use criterion::{Criterion, criterion_group, criterion_main};
use simplicio_code_gateway::{parse_sse_events, redact_diagnostics};

fn bench_contract_hot_paths(c: &mut Criterion) {
    let payload = serde_json::json!({
        "status": 429,
        "request_id": "request-1",
        "prompt": "redacted prompt",
        "code": "redacted code",
        "authorization": "redacted token",
        "retry_after": 3
    });
    let sse = br#"data: {"id":"r","text_delta":"x","tool_call":null,"usage":null,"done":false}

data: [DONE]

"#;
    c.bench_function("redact_gateway_diagnostics", |b| {
        b.iter(|| redact_diagnostics(&payload))
    });
    c.bench_function("parse_gateway_sse", |b| b.iter(|| parse_sse_events(sse)));
}

criterion_group!(benches, bench_contract_hot_paths);
criterion_main!(benches);
