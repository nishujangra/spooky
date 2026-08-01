use serde_json::json;

pub const CONTROL_API_AUDIT_LOG_TARGET: &str = "spooky.control_api.audit";

pub fn should_passthrough_raw_json_target(target: &str) -> bool {
    target == CONTROL_API_AUDIT_LOG_TARGET
}

pub fn build_json_payload(ts: &str, level: &str, target: &str, message: &str) -> serde_json::Value {
    json!({
        "ts": ts,
        "level": level,
        "target": target,
        "msg": message,
    })
}
