use crate::config::{CURRENT_CONFIG_VERSION, LoadBalancing, Log, LogFile, LogFormat};

// default values
pub fn get_default_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

// Listen defaults
pub(crate) fn get_default_protocol() -> String {
    String::from("http3")
}

pub(crate) fn get_default_port() -> u16 {
    9889
}

pub(crate) fn get_default_address() -> String {
    String::from("0.0.0.0")
}

// Backend defaults
pub(crate) fn get_default_weight() -> u32 {
    100
}

// Health-check defaults
pub(crate) fn get_default_path() -> String {
    String::from("/health")
}

pub(crate) fn get_default_interval() -> u64 {
    5000
}

pub(crate) fn get_default_health_timeout() -> u64 {
    1000
}

pub(crate) fn get_default_failure_threshold() -> u32 {
    3
}

pub(crate) fn get_default_success_threshold() -> u32 {
    2
}

pub(crate) fn get_default_cooldown_ms() -> u64 {
    5_000
}

// Log and LogFile defaults
pub(crate) fn get_default_log_level() -> String {
    String::from("info")
}

pub(crate) fn get_default_log_file_path() -> String {
    String::from("/var/log/spooky/spooky.log")
}

pub fn get_default_load_balancing() -> LoadBalancing {
    LoadBalancing {
        lb_type: String::from("round-robin"),
        key: None,
    }
}

pub(crate) fn get_default_log() -> Log {
    Log {
        level: String::from("info"),
        file: LogFile {
            enabled: false,
            path: get_default_log_file_path(),
        },
        format: LogFormat::Plain,
    }
}

// PrivilegeDrop defaults
pub(crate) fn security_default_drop_privileges() -> bool {
    true
}

pub(crate) fn security_default_user() -> String {
    String::from("nobody")
}

pub(crate) fn security_default_group() -> String {
    String::from("nogroup")
}

pub fn auth_default_api_key_header_name() -> String {
    String::from("x-api-key")
}

pub fn auth_default_external_timeout_ms() -> u64 {
    1_000
}

pub fn auth_default_external_failure_mode() -> String {
    String::from("fail_closed")
}

pub fn auth_default_jwt_clock_skew_secs() -> u64 {
    30
}

pub fn perf_default_worker_threads() -> usize {
    1
}

pub fn perf_default_control_plane_threads() -> usize {
    2
}

pub fn perf_default_packet_shards_per_worker() -> usize {
    1
}

pub fn perf_default_packet_shard_queue_capacity() -> usize {
    2048
}

pub fn perf_default_packet_shard_queue_max_bytes() -> usize {
    64 * 1024 * 1024
}

pub fn perf_default_reuseport() -> bool {
    true
}

pub fn perf_default_pin_workers() -> bool {
    false
}

pub fn perf_default_global_inflight_limit() -> usize {
    4096
}

pub fn perf_default_per_upstream_inflight_limit() -> usize {
    1024
}

pub fn perf_default_inflight_acquire_wait_ms() -> u64 {
    0
}

pub fn perf_default_backend_timeout_ms() -> u64 {
    2_000
}

pub fn perf_default_backend_connect_timeout_ms() -> u64 {
    500
}

pub fn perf_default_backend_body_idle_timeout_ms() -> u64 {
    2_000
}

pub fn perf_default_backend_body_total_timeout_ms() -> u64 {
    30_000
}

pub fn perf_default_backend_total_request_timeout_ms() -> u64 {
    35_000
}

pub fn perf_default_shutdown_drain_timeout_ms() -> u64 {
    5_000
}

pub fn perf_default_udp_recv_buffer_bytes() -> usize {
    8 * 1024 * 1024
}

pub fn perf_default_udp_send_buffer_bytes() -> usize {
    8 * 1024 * 1024
}

pub fn perf_default_h2_pool_max_idle_per_backend() -> usize {
    256
}

pub fn perf_default_h2_pool_idle_timeout_ms() -> u64 {
    90_000
}

pub fn perf_default_backend_dns_refresh_enabled() -> bool {
    false
}

pub fn perf_default_backend_dns_refresh_interval_ms() -> u64 {
    30_000
}

pub fn perf_default_per_backend_inflight_limit() -> usize {
    64
}

pub fn perf_default_new_connections_per_sec() -> u32 {
    2000
}

pub fn perf_default_new_connections_burst() -> u32 {
    500
}

pub fn perf_default_max_active_connections() -> usize {
    20_000
}

pub fn perf_default_quic_max_idle_timeout_ms() -> u64 {
    5_000
}

pub fn perf_default_quic_initial_max_data() -> u64 {
    10_000_000
}

pub fn perf_default_quic_initial_max_stream_data() -> u64 {
    1_000_000
}

pub fn perf_default_quic_initial_max_streams_bidi() -> u64 {
    100
}

pub fn perf_default_quic_initial_max_streams_uni() -> u64 {
    100
}

pub fn perf_default_max_response_body_bytes() -> usize {
    100 * 1024 * 1024 // 100 MiB
}

pub fn perf_default_max_request_body_bytes() -> usize {
    1_000_000 // 1 MiB
}

pub fn perf_default_request_buffer_global_cap_bytes() -> usize {
    64 * 1024 * 1024 // 64 MiB
}

pub fn resilience_default_scoped_rate_limit_idle_ttl_secs() -> u64 {
    300
}

pub fn perf_default_unknown_length_response_prebuffer_bytes() -> usize {
    2 * 1024 * 1024 // 2 MiB
}

pub fn perf_default_client_body_idle_timeout_ms() -> u64 {
    10_000
}

pub fn resilience_default_adaptive_enabled() -> bool {
    true
}

pub fn resilience_default_adaptive_min_limit() -> usize {
    64
}

pub fn resilience_default_adaptive_decrease_step() -> usize {
    16
}

pub fn resilience_default_adaptive_increase_step() -> usize {
    16
}

pub fn resilience_default_adaptive_high_latency_ms() -> u64 {
    500
}

pub fn resilience_default_route_queue_default_cap() -> usize {
    512
}

pub fn resilience_default_route_queue_global_cap() -> usize {
    2048
}

pub fn resilience_default_route_queue_shed_retry_after_seconds() -> u32 {
    1
}

pub fn resilience_default_protocol_allow_0rtt() -> bool {
    false
}

pub fn resilience_default_protocol_max_headers_count() -> usize {
    128
}

pub fn resilience_default_protocol_max_headers_bytes() -> usize {
    16 * 1024
}

pub fn resilience_default_protocol_enforce_authority_host_match() -> bool {
    true
}

pub fn resilience_default_protocol_allow_connect() -> bool {
    false
}

pub fn resilience_default_cb_enabled() -> bool {
    true
}

pub fn resilience_default_cb_failure_threshold() -> u32 {
    3
}

pub fn resilience_default_cb_open_ms() -> u64 {
    30_000
}

pub fn resilience_default_cb_half_open_max_probes() -> u32 {
    1
}

pub fn resilience_default_hedging_enabled() -> bool {
    false
}

pub fn resilience_default_hedging_delay_ms() -> u64 {
    100
}

pub fn resilience_default_retry_budget_enabled() -> bool {
    true
}

pub fn resilience_default_retry_budget_ratio_percent() -> u8 {
    10
}

pub fn resilience_default_brownout_enabled() -> bool {
    true
}

pub fn resilience_default_brownout_trigger_inflight_percent() -> u8 {
    90
}

pub fn resilience_default_brownout_recover_inflight_percent() -> u8 {
    60
}

pub fn resilience_default_watchdog_enabled() -> bool {
    false
}

pub fn resilience_default_watchdog_check_interval_ms() -> u64 {
    1_000
}

pub fn resilience_default_watchdog_poll_stall_timeout_ms() -> u64 {
    5_000
}

pub fn resilience_default_watchdog_timeout_error_rate_percent() -> u8 {
    60
}

pub fn resilience_default_watchdog_min_requests_per_window() -> u64 {
    20
}

pub fn resilience_default_watchdog_overload_inflight_percent() -> u8 {
    95
}

pub fn resilience_default_watchdog_unhealthy_consecutive_windows() -> u32 {
    3
}

pub fn resilience_default_watchdog_drain_grace_ms() -> u64 {
    8_000
}

pub fn resilience_default_watchdog_restart_cooldown_ms() -> u64 {
    120_000
}

pub fn observe_default_address() -> String {
    String::from("127.0.0.1")
}

pub fn observe_default_port() -> u16 {
    9901
}

pub fn observe_default_metrics_path() -> String {
    String::from("/metrics")
}

pub fn observe_default_metrics_max_connections() -> usize {
    512
}

pub fn observe_default_metrics_connection_timeout_ms() -> u64 {
    30_000
}

pub fn observe_default_control_api_address() -> String {
    String::from("127.0.0.1")
}

pub fn observe_default_control_api_port() -> u16 {
    9902
}

pub fn observe_default_control_api_health_path() -> String {
    String::from("/health")
}

pub fn observe_default_control_api_ready_path() -> String {
    String::from("/ready")
}

pub fn observe_default_control_api_runtime_path() -> String {
    String::from("/admin/runtime")
}

pub fn observe_default_control_api_restart_path() -> String {
    String::from("/admin/runtime/restart")
}

pub fn observe_default_control_api_reload_path() -> String {
    String::from("/admin/runtime/reload")
}

pub fn observe_default_control_api_reload_certs_path() -> String {
    String::from("/admin/runtime/reload-certs")
}

pub fn observe_default_control_api_max_connections() -> usize {
    256
}

pub fn observe_default_control_api_connection_timeout_ms() -> u64 {
    30_000
}

pub fn observe_default_tracing_service_name() -> String {
    String::from("spooky")
}

pub fn observe_default_tracing_sample_ratio() -> f64 {
    1.0
}

pub fn observe_default_routing_transparency_enabled() -> bool {
    false
}

pub fn observe_default_routing_transparency_include_reason() -> bool {
    true
}

pub fn observe_default_routing_transparency_expose_header() -> bool {
    false
}

pub fn observe_default_routing_transparency_header_name() -> String {
    String::from("x-spooky-route-decision")
}

pub fn upstream_tls_default_verify_certificates() -> bool {
    true
}

pub fn upstream_tls_default_strict_sni() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogFormat;

    #[test]
    fn documented_scalar_defaults_match_contract() {
        assert_eq!(get_default_version(), 1);
        assert_eq!(get_default_protocol(), "http3");
        assert_eq!(get_default_port(), 9889);
        assert_eq!(get_default_address(), "0.0.0.0");
        assert_eq!(get_default_weight(), 100);
        assert_eq!(get_default_path(), "/health");
        assert_eq!(get_default_interval(), 5_000);
        assert_eq!(get_default_health_timeout(), 1_000);
        assert_eq!(get_default_failure_threshold(), 3);
        assert_eq!(get_default_success_threshold(), 2);
        assert_eq!(get_default_log_level(), "info");
    }

    #[test]
    fn documented_composite_defaults_match_contract() {
        let load_balancing = get_default_load_balancing();
        assert_eq!(load_balancing.lb_type, "round-robin");
        assert_eq!(load_balancing.key, None);

        let log = get_default_log();
        assert_eq!(log.level, "info");
        assert!(!log.file.enabled);
        assert_eq!(log.file.path, "/var/log/spooky/spooky.log");
        assert_eq!(log.format, LogFormat::Plain);
    }
}
