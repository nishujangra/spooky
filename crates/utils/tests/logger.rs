use std::{
    io,
    path::Path,
    sync::{Mutex, OnceLock},
};

use log::LevelFilter;
use serde_json::json;
use impulse_utils::logger::{
    CONTROL_API_AUDIT_LOG_TARGET,
    errors::{build_create_log_dir_error, build_open_log_file_error},
    formatter::{build_json_payload, should_passthrough_raw_json_target},
    init::{LoggerInitStatus, try_init_logger},
    set_log_level,
};

fn logger_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn logger_init_is_idempotent() {
    let _guard = logger_test_guard();
    let _first = try_init_logger("info", false, "", false);

    let second = try_init_logger("debug", true, "/tmp/ignored.log", true);
    assert_eq!(second, LoggerInitStatus::AlreadyInitialized);
}

#[test]
fn set_log_level_accepts_all_supported_aliases() {
    let _guard = logger_test_guard();

    let cases = [
        ("whisper", LevelFilter::Trace),
        ("trace", LevelFilter::Trace),
        ("haunt", LevelFilter::Debug),
        ("debug", LevelFilter::Debug),
        ("impulse", LevelFilter::Info),
        ("info", LevelFilter::Info),
        ("scream", LevelFilter::Warn),
        ("warn", LevelFilter::Warn),
        ("poltergeist", LevelFilter::Error),
        ("error", LevelFilter::Error),
        ("silence", LevelFilter::Off),
        ("off", LevelFilter::Off),
    ];

    for (alias, expected) in cases {
        set_log_level(alias).expect("supported log level should be accepted");
        assert_eq!(
            log::max_level(),
            expected,
            "alias {alias} mapped incorrectly"
        );
    }
}

#[test]
fn set_log_level_rejects_invalid_values() {
    let _guard = logger_test_guard();

    let err = set_log_level("debug-verbose").expect_err("invalid level must be rejected");
    assert_eq!(err.to_string(), "invalid log level 'debug-verbose'");
}

#[test]
fn create_log_dir_error_includes_directory_and_file_path() {
    let err = io::Error::new(io::ErrorKind::PermissionDenied, "permission denied");
    let msg = build_create_log_dir_error(
        "/var/log/impulse/impulse.log",
        Path::new("/var/log/impulse"),
        &err,
    );

    assert!(msg.contains("/var/log/impulse"));
    assert!(msg.contains("/var/log/impulse/impulse.log"));
    assert!(msg.contains("permission denied"));
}

#[test]
fn open_log_file_error_includes_file_path() {
    let err = io::Error::new(io::ErrorKind::PermissionDenied, "permission denied");
    let msg = build_open_log_file_error("/var/log/impulse/impulse.log", &err);

    assert!(msg.contains("/var/log/impulse/impulse.log"));
    assert!(msg.contains("permission denied"));
}

#[test]
fn json_payload_preserves_plain_message_verbatim() {
    let payload = build_json_payload("123", "info", "impulse", "request_id=42 status=200");

    assert_eq!(
        payload,
        json!({
            "ts": "123",
            "level": "info",
            "target": "impulse",
            "msg": "request_id=42 status=200",
        })
    );
}

#[test]
fn json_payload_preserves_malformed_kv_like_message_verbatim() {
    let payload = build_json_payload(
        "123",
        "warn",
        "impulse_edge",
        r#"request_id= status="" path=, trace_id==broken"#,
    );

    assert_eq!(
        payload,
        json!({
            "ts": "123",
            "level": "warn",
            "target": "impulse_edge",
            "msg": r#"request_id= status="" path=, trace_id==broken"#,
        })
    );
}

#[test]
fn json_payload_preserves_embedded_quotes_and_whitespace() {
    let payload = build_json_payload(
        "123",
        "error",
        "impulse_edge::quic_listener::forwarding",
        r#"msg="backend failed" path="/api v1" detail="x=y, z""#,
    );

    assert_eq!(
        payload,
        json!({
            "ts": "123",
            "level": "error",
            "target": "impulse_edge::quic_listener::forwarding",
            "msg": r#"msg="backend failed" path="/api v1" detail="x=y, z""#,
        })
    );
}

#[test]
fn control_api_audit_target_is_marked_for_raw_json_passthrough() {
    assert!(should_passthrough_raw_json_target(
        CONTROL_API_AUDIT_LOG_TARGET
    ));
    assert!(!should_passthrough_raw_json_target("impulse_edge"));
}
