use serde_json::{Map, Value};

/// Metadata-only diagnostics. Prompt, code, response, tool arguments, and
/// credential-shaped values are removed before anything can be logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RedactedDiagnostics {
    pub message: String,
    pub fields: Vec<(String, String)>,
}

pub fn redact_diagnostics(value: &Value) -> RedactedDiagnostics {
    let mut fields = Vec::new();
    let sanitized = sanitize(value, &mut fields);
    let message = sanitized
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("private gateway request failed")
        .to_owned();
    RedactedDiagnostics { message, fields }
}

fn sanitize(value: &Value, fields: &mut Vec<(String, String)>) -> Value {
    let Value::Object(object) = value else {
        return Value::Null;
    };
    let mut output = Map::new();
    for (key, value) in object {
        let lower = key.to_ascii_lowercase();
        if lower.contains("prompt")
            || lower.contains("content")
            || lower.contains("code")
            || lower.contains("response")
            || lower.contains("argument")
            || lower.contains("authorization")
            || lower.contains("token")
            || lower.contains("secret")
            || lower.contains("api_key")
        {
            continue;
        }
        match value {
            Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {
                output.insert(key.clone(), value.clone());
            }
            Value::Array(_) | Value::Object(_) => {
                output.insert(key.clone(), sanitize(value, fields));
            }
        }
    }
    for (key, value) in &output {
        let rendered = match value {
            Value::String(_) => "string".to_owned(),
            Value::Number(_) => "number".to_owned(),
            Value::Bool(_) => "bool".to_owned(),
            Value::Null => "null".to_owned(),
            Value::Array(items) => format!("array:{}", items.len()),
            Value::Object(items) => format!("object:{}", items.len()),
        };
        fields.push((key.clone(), rendered));
    }
    Value::Object(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_payloads_and_secrets_but_keeps_metadata() {
        let value = serde_json::json!({
            "status": 429,
            "request_id": "req-1",
            "prompt": "do not retain",
            "code": "secret source",
            "response": "secret answer",
            "authorization": "Bearer secret",
            "nested": {"tool_arguments": "do not retain", "retry_after": 3}
        });
        let redacted = redact_diagnostics(&value);
        assert!(!redacted.message.contains("secret"));
        let joined = format!("{:?}", redacted.fields);
        assert!(joined.contains("status"));
        assert!(!joined.contains("prompt"));
        assert!(!joined.contains("secret"));
    }
}
