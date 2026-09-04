use serde_json::json;

pub const CONTROL_API_AUDIT_LOG_TARGET: &str = "impulse.control_api.audit";

pub fn should_passthrough_raw_json_target(target: &str) -> bool {
    target == CONTROL_API_AUDIT_LOG_TARGET
}

/// Convert untrusted log content into a single terminal-safe line.
///
/// ANSI escape sequences are removed and control characters are represented
/// visibly so request-derived values cannot forge log lines or terminal output.
pub fn sanitize_log_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.next() == Some('[') {
                for sequence_char in chars.by_ref() {
                    if ('@'..='~').contains(&sequence_char) {
                        break;
                    }
                }
            }
            continue;
        }

        match ch {
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write;
                let _ = write!(sanitized, "\\u{{{:04x}}}", ch as u32);
            }
            _ => sanitized.push(ch),
        }
    }

    sanitized
}

pub fn build_json_payload(ts: &str, level: &str, target: &str, message: &str) -> serde_json::Value {
    json!({
        "ts": ts,
        "level": level,
        "target": target,
        "msg": message,
    })
}
