//! Policy-combination and route-matcher rejection cases.

use spooky_config::config::UpstreamHostPolicyMode;

use crate::common::{
    api_upstream_mut, assert_config_error_contains, runtime_config_err, sample_config,
};

#[test]
fn runtime_config_rejects_host_override_when_host_policy_is_not_rewrite() {
    let mut config = sample_config();
    let upstream = api_upstream_mut(&mut config);
    upstream.host_policy.mode = UpstreamHostPolicyMode::Upstream;
    upstream.host_policy.host = Some("ignored.example.com".to_string());

    let err = runtime_config_err(&config);
    assert_config_error_contains(
        &err,
        "unsupported_policy_combination",
        "mode is not rewrite",
    );
}

#[test]
fn runtime_config_rejects_duplicate_route_matchers_across_upstreams() {
    let mut config = sample_config();
    config.upstream.insert(
        "api-copy".to_string(),
        config
            .upstream
            .get("api")
            .expect("shared regression fixture must include the 'api' upstream")
            .clone(),
    );

    let err = runtime_config_err(&config);
    assert_config_error_contains(&err, "duplicate_route_ambiguity", "conflicts with upstream");
}

#[test]
fn runtime_config_rejects_invalid_request_key_spec() {
    let mut config = sample_config();
    api_upstream_mut(&mut config).load_balancing.key = Some("header:   ".to_string());

    let err = runtime_config_err(&config);
    assert_config_error_contains(&err, "config_invalid", "unsupported request key spec");
}

#[test]
fn runtime_config_rejects_connect_route_when_protocol_policy_disallows_it() {
    let mut config = sample_config();
    api_upstream_mut(&mut config).route.method = Some("CONNECT".to_string());
    config.resilience.protocol.allow_connect = false;

    let err = runtime_config_err(&config);
    assert_config_error_contains(
        &err,
        "unsupported_policy_combination",
        "allow_connect=false",
    );
}
