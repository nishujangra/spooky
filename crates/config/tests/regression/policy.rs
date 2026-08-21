//! Policy-combination and route-matcher rejection cases.

use impulse_config::config::UpstreamHostPolicyMode;

use crate::common::{
    api_upstream_mut, assert_config_error_contains, duplicate_api_upstream,
    sample_runtime_config_err_with,
};

#[test]
fn runtime_config_rejects_host_override_when_host_policy_is_not_rewrite() {
    let err = sample_runtime_config_err_with(|config| {
        let upstream = api_upstream_mut(config);
        upstream.host_policy.mode = UpstreamHostPolicyMode::Upstream;
        upstream.host_policy.host = Some("ignored.example.com".to_string());
    });
    assert_config_error_contains(
        &err,
        "unsupported_policy_combination",
        "mode is not rewrite",
    );
}

#[test]
fn runtime_config_rejects_duplicate_route_matchers_across_upstreams() {
    let err = sample_runtime_config_err_with(|config| {
        duplicate_api_upstream(config, "api-copy");
    });
    assert_config_error_contains(&err, "duplicate_route_ambiguity", "conflicts with upstream");
}

#[test]
fn runtime_config_rejects_invalid_request_key_spec() {
    let err = sample_runtime_config_err_with(|config| {
        api_upstream_mut(config).load_balancing.key = Some("header:   ".to_string());
    });
    assert_config_error_contains(&err, "config_invalid", "unsupported request key spec");
}

#[test]
fn runtime_config_rejects_connect_route_when_protocol_policy_disallows_it() {
    let err = sample_runtime_config_err_with(|config| {
        api_upstream_mut(config).route.method = Some("CONNECT".to_string());
        config.resilience.protocol.allow_connect = false;
    });
    assert_config_error_contains(
        &err,
        "unsupported_policy_combination",
        "allow_connect=false",
    );
}
