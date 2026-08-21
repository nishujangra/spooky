use impulse_config::{
    backend_endpoint::BackendEndpoint,
    config::{UpstreamHostPolicy, UpstreamHostPolicyMode},
};
use impulse_errors::BridgeError;

pub fn resolve_upstream_host_value<'a>(
    endpoint: &'a BackendEndpoint,
    host_policy: &'a UpstreamHostPolicy,
    request_authority: Option<&'a str>,
    host_header: Option<&'a str>,
) -> Result<&'a str, BridgeError> {
    match host_policy.mode {
        UpstreamHostPolicyMode::PassThrough => Ok(request_authority
            .or(host_header)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(endpoint.authority())),
        UpstreamHostPolicyMode::Rewrite => host_policy
            .host
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(BridgeError::InvalidHeader),
        UpstreamHostPolicyMode::Upstream => Ok(endpoint.authority()),
    }
}
