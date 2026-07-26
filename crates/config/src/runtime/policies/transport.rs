use std::time::Duration;

use super::{RuntimeBackendDnsPolicy, config_invalid};
use crate::{config::Performance, runtime::RuntimeConfigError};

fn require_nonzero_u64(name: &str, value: u64) -> Result<(), RuntimeConfigError> {
    if value == 0 {
        return Err(config_invalid(format!("{name} must be greater than 0")));
    }
    Ok(())
}

fn require_nonzero_usize(name: &str, value: usize) -> Result<(), RuntimeConfigError> {
    if value == 0 {
        return Err(config_invalid(format!("{name} must be greater than 0")));
    }
    Ok(())
}

fn require_nonzero_u32(name: &str, value: u32) -> Result<(), RuntimeConfigError> {
    if value == 0 {
        return Err(config_invalid(format!("{name} must be greater than 0")));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTransportPolicy {
    pub worker_threads: usize,
    pub control_plane_threads: usize,
    pub packet_shards_per_worker: usize,
    pub packet_shard_queue_capacity: usize,
    pub packet_shard_queue_max_bytes: usize,
    pub reuseport: bool,
    pub pin_workers: bool,
    pub global_inflight_limit: usize,
    pub per_upstream_inflight_limit: usize,
    pub per_backend_inflight_limit: usize,
    pub udp_recv_buffer_bytes: usize,
    pub udp_send_buffer_bytes: usize,
    pub h2_pool_max_idle_per_backend: usize,
    pub backend_dns_refresh_enabled: bool,
    pub new_connections_per_sec: u32,
    pub new_connections_burst: u32,
    pub max_active_connections: usize,
    pub quic_initial_max_data: u64,
    pub quic_initial_max_stream_data: u64,
    pub quic_initial_max_streams_bidi: u64,
    pub quic_initial_max_streams_uni: u64,
    pub max_response_body_bytes: usize,
    pub max_request_body_bytes: usize,
    pub request_buffer_global_cap_bytes: usize,
    pub unknown_length_response_prebuffer_bytes: usize,
    pub connection_limits: RuntimeConnectionLimits,
    pub backend_connections: RuntimeBackendConnectionPolicy,
    pub backend_dns: RuntimeBackendDnsPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConnectionLimits {
    pub global_inflight: usize,
    pub per_upstream_inflight: usize,
    pub per_backend: usize,
    pub backend_pool_max_inflight: usize,
    pub max_active_connections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendConnectionPolicy {
    pub max_inflight: usize,
    pub max_idle_per_backend: usize,
    pub pool_idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub execution_timeout: Duration,
}

impl RuntimeTransportPolicy {
    pub(crate) fn normalize(performance: &Performance) -> Result<Self, RuntimeConfigError> {
        require_nonzero_usize("performance.worker_threads", performance.worker_threads)?;
        if performance.worker_threads > 1024 {
            return Err(config_invalid(format!(
                "performance.worker_threads={} exceeds the maximum of 1024",
                performance.worker_threads
            )));
        }
        require_nonzero_usize(
            "performance.control_plane_threads",
            performance.control_plane_threads,
        )?;
        if performance.control_plane_threads > 1024 {
            return Err(config_invalid(format!(
                "performance.control_plane_threads={} exceeds the maximum of 1024",
                performance.control_plane_threads
            )));
        }
        require_nonzero_usize(
            "performance.packet_shards_per_worker",
            performance.packet_shards_per_worker,
        )?;
        if performance.packet_shards_per_worker > 256 {
            return Err(config_invalid(format!(
                "performance.packet_shards_per_worker={} exceeds the maximum of 256",
                performance.packet_shards_per_worker
            )));
        }
        require_nonzero_usize(
            "performance.packet_shard_queue_capacity",
            performance.packet_shard_queue_capacity,
        )?;
        require_nonzero_usize(
            "performance.packet_shard_queue_max_bytes",
            performance.packet_shard_queue_max_bytes,
        )?;
        if performance.worker_threads > 1 && !performance.reuseport {
            return Err(config_invalid(
                "performance.reuseport must be true when performance.worker_threads > 1",
            ));
        }
        require_nonzero_usize(
            "performance.global_inflight_limit",
            performance.global_inflight_limit,
        )?;
        require_nonzero_usize(
            "performance.per_upstream_inflight_limit",
            performance.per_upstream_inflight_limit,
        )?;
        require_nonzero_usize(
            "performance.per_backend_inflight_limit",
            performance.per_backend_inflight_limit,
        )?;
        require_nonzero_usize(
            "performance.udp_recv_buffer_bytes",
            performance.udp_recv_buffer_bytes,
        )?;
        require_nonzero_usize(
            "performance.udp_send_buffer_bytes",
            performance.udp_send_buffer_bytes,
        )?;
        require_nonzero_usize(
            "performance.h2_pool_max_idle_per_backend",
            performance.h2_pool_max_idle_per_backend,
        )?;
        require_nonzero_u32(
            "performance.new_connections_per_sec",
            performance.new_connections_per_sec,
        )?;
        require_nonzero_u32(
            "performance.new_connections_burst",
            performance.new_connections_burst,
        )?;
        require_nonzero_usize(
            "performance.max_active_connections",
            performance.max_active_connections,
        )?;
        require_nonzero_u64(
            "performance.quic_initial_max_data",
            performance.quic_initial_max_data,
        )?;
        require_nonzero_u64(
            "performance.quic_initial_max_stream_data",
            performance.quic_initial_max_stream_data,
        )?;
        if performance.quic_initial_max_stream_data > performance.quic_initial_max_data {
            return Err(config_invalid(format!(
                "performance.quic_initial_max_stream_data ({}) must be <= quic_initial_max_data ({})",
                performance.quic_initial_max_stream_data, performance.quic_initial_max_data
            )));
        }
        require_nonzero_u64(
            "performance.quic_initial_max_streams_bidi",
            performance.quic_initial_max_streams_bidi,
        )?;
        require_nonzero_u64(
            "performance.quic_initial_max_streams_uni",
            performance.quic_initial_max_streams_uni,
        )?;
        require_nonzero_usize(
            "performance.max_response_body_bytes",
            performance.max_response_body_bytes,
        )?;
        require_nonzero_usize(
            "performance.max_request_body_bytes",
            performance.max_request_body_bytes,
        )?;
        require_nonzero_usize(
            "performance.request_buffer_global_cap_bytes",
            performance.request_buffer_global_cap_bytes,
        )?;
        require_nonzero_usize(
            "performance.unknown_length_response_prebuffer_bytes",
            performance.unknown_length_response_prebuffer_bytes,
        )?;
        if performance.max_request_body_bytes > performance.quic_initial_max_stream_data as usize {
            return Err(config_invalid(format!(
                "performance.max_request_body_bytes ({}) must be <= quic_initial_max_stream_data ({})",
                performance.max_request_body_bytes, performance.quic_initial_max_stream_data
            )));
        }
        if performance.request_buffer_global_cap_bytes < performance.max_request_body_bytes {
            return Err(config_invalid(format!(
                "performance.request_buffer_global_cap_bytes ({}) must be >= max_request_body_bytes ({})",
                performance.request_buffer_global_cap_bytes, performance.max_request_body_bytes
            )));
        }
        if performance.unknown_length_response_prebuffer_bytes > performance.max_response_body_bytes
        {
            return Err(config_invalid(format!(
                "performance.unknown_length_response_prebuffer_bytes ({}) must be <= max_response_body_bytes ({})",
                performance.unknown_length_response_prebuffer_bytes,
                performance.max_response_body_bytes
            )));
        }

        Ok(Self {
            worker_threads: performance.worker_threads,
            control_plane_threads: performance.control_plane_threads,
            packet_shards_per_worker: performance.packet_shards_per_worker,
            packet_shard_queue_capacity: performance.packet_shard_queue_capacity,
            packet_shard_queue_max_bytes: performance.packet_shard_queue_max_bytes,
            reuseport: performance.reuseport,
            pin_workers: performance.pin_workers,
            global_inflight_limit: performance.global_inflight_limit,
            per_upstream_inflight_limit: performance.per_upstream_inflight_limit,
            per_backend_inflight_limit: performance.per_backend_inflight_limit,
            udp_recv_buffer_bytes: performance.udp_recv_buffer_bytes,
            udp_send_buffer_bytes: performance.udp_send_buffer_bytes,
            h2_pool_max_idle_per_backend: performance.h2_pool_max_idle_per_backend,
            backend_dns_refresh_enabled: performance.backend_dns_refresh_enabled,
            new_connections_per_sec: performance.new_connections_per_sec,
            new_connections_burst: performance.new_connections_burst,
            max_active_connections: performance.max_active_connections,
            quic_initial_max_data: performance.quic_initial_max_data,
            quic_initial_max_stream_data: performance.quic_initial_max_stream_data,
            quic_initial_max_streams_bidi: performance.quic_initial_max_streams_bidi,
            quic_initial_max_streams_uni: performance.quic_initial_max_streams_uni,
            max_response_body_bytes: performance.max_response_body_bytes,
            max_request_body_bytes: performance.max_request_body_bytes,
            request_buffer_global_cap_bytes: performance.request_buffer_global_cap_bytes,
            unknown_length_response_prebuffer_bytes: performance
                .unknown_length_response_prebuffer_bytes,
            connection_limits: RuntimeConnectionLimits {
                global_inflight: performance.global_inflight_limit,
                per_upstream_inflight: performance.per_upstream_inflight_limit,
                per_backend: performance.per_backend_inflight_limit,
                backend_pool_max_inflight: performance
                    .per_backend_inflight_limit
                    .saturating_mul(performance.worker_threads.max(1)),
                max_active_connections: performance.max_active_connections,
            },
            backend_connections: RuntimeBackendConnectionPolicy {
                max_inflight: performance
                    .per_backend_inflight_limit
                    .saturating_mul(performance.worker_threads.max(1)),
                max_idle_per_backend: performance.h2_pool_max_idle_per_backend,
                pool_idle_timeout: Duration::from_millis(performance.h2_pool_idle_timeout_ms),
                connect_timeout: Duration::from_millis(performance.backend_connect_timeout_ms),
                execution_timeout: Duration::from_millis(performance.backend_timeout_ms),
            },
            backend_dns: RuntimeBackendDnsPolicy::from_performance(performance),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Performance;

    #[test]
    fn normalize_rejects_unknown_length_prebuffer_above_response_cap() {
        let mut performance = Performance::default();
        performance.max_response_body_bytes = 1024;
        performance.unknown_length_response_prebuffer_bytes = 1025;

        let err =
            RuntimeTransportPolicy::normalize(&performance).expect_err("normalization must fail");

        assert!(
            err.to_string().contains(
                "performance.unknown_length_response_prebuffer_bytes (1025) must be <= max_response_body_bytes (1024)"
            )
        );
    }

    #[test]
    fn normalize_rejects_request_body_cap_above_stream_flow_control() {
        let mut performance = Performance::default();
        performance.quic_initial_max_stream_data = 4096;
        performance.max_request_body_bytes = 4097;
        performance.request_buffer_global_cap_bytes = 4097;

        let err =
            RuntimeTransportPolicy::normalize(&performance).expect_err("normalization must fail");

        assert!(
            err.to_string().contains(
                "performance.max_request_body_bytes (4097) must be <= quic_initial_max_stream_data (4096)"
            )
        );
    }

    #[test]
    fn normalize_rejects_request_buffer_cap_below_request_body_cap() {
        let mut performance = Performance::default();
        performance.max_request_body_bytes = 8192;
        performance.request_buffer_global_cap_bytes = 8191;
        performance.quic_initial_max_stream_data = 8192;

        let err =
            RuntimeTransportPolicy::normalize(&performance).expect_err("normalization must fail");

        assert!(
            err.to_string().contains(
                "performance.request_buffer_global_cap_bytes (8191) must be >= max_request_body_bytes (8192)"
            )
        );
    }

    #[test]
    fn normalize_preserves_guardrail_values_when_limits_are_consistent() {
        let mut performance = Performance::default();
        performance.worker_threads = 1;
        performance.reuseport = false;
        performance.quic_initial_max_data = 65_536;
        performance.quic_initial_max_stream_data = 16_384;
        performance.max_request_body_bytes = 16_384;
        performance.request_buffer_global_cap_bytes = 20_000;
        performance.max_response_body_bytes = 32_768;
        performance.unknown_length_response_prebuffer_bytes = 8_192;

        let policy =
            RuntimeTransportPolicy::normalize(&performance).expect("normalization must succeed");

        assert_eq!(policy.max_request_body_bytes, 16_384);
        assert_eq!(policy.request_buffer_global_cap_bytes, 20_000);
        assert_eq!(policy.max_response_body_bytes, 32_768);
        assert_eq!(policy.unknown_length_response_prebuffer_bytes, 8_192);
    }
}
