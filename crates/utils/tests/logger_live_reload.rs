use std::fs;

use impulse_utils::logger::{LoggerInitStatus, set_log_level, try_init_logger};
use tempfile::tempdir;

#[test]
fn raising_log_level_live_enables_new_debug_emission() {
    let dir = tempdir().expect("tempdir");
    let log_path = dir.path().join("impulse.log");
    let log_path_str = log_path.to_string_lossy().to_string();

    let init = try_init_logger("info", true, &log_path_str, false);
    assert_eq!(init, LoggerInitStatus::Initialized);

    let debug_before = "debug-before-live-raise";
    let info_marker = "info-marker-before-live-raise";
    let debug_after = "debug-after-live-raise";

    log::debug!("{debug_before}");
    log::info!("{info_marker}");
    log::logger().flush();

    let initial_contents = fs::read_to_string(&log_path).expect("read initial log file");
    assert!(
        !initial_contents.contains(debug_before),
        "debug log should be blocked before live raise: {initial_contents}"
    );
    assert!(
        initial_contents.contains(info_marker),
        "info log should be emitted before live raise: {initial_contents}"
    );

    set_log_level("debug").expect("raise log level to debug");

    log::debug!("{debug_after}");
    log::logger().flush();

    let updated_contents = fs::read_to_string(&log_path).expect("read updated log file");
    assert!(
        updated_contents.contains(debug_after),
        "debug log should be emitted after live raise: {updated_contents}"
    );
}
