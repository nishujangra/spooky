//! Shared request-building fixtures for the bridge regression suite.

use std::{convert::Infallible, net::SocketAddr};

use bytes::Bytes;
use http::HeaderMap;
use http_body_util::{BodyExt, Empty, combinators::BoxBody};
use quiche::h3::Header;
use spooky_bridge::request::{
    RequestBodyMode, RequestBuildInput, RequestBuildPolicies, RequestBuildTarget,
    RequestForwardedContext, RequestTraceContext, build_h1_request, build_h2_request_for_target,
};
use spooky_config::{
    backend_endpoint::BackendEndpoint,
    config::{ForwardedHeaderPolicy, UpstreamHostPolicy},
};
use spooky_errors::BridgeError;

pub type CanonicalBridgeRequest = http::Request<BoxBody<Bytes, Infallible>>;
pub type CanonicalBridgeRequestPair = (CanonicalBridgeRequest, CanonicalBridgeRequest);

#[derive(Clone, Copy)]
pub struct RequestInputMeta<'a> {
    pub authority: Option<&'a str>,
    pub content_length: Option<usize>,
    pub request_id: u64,
    pub traceparent: Option<&'a str>,
    pub client_addr: SocketAddr,
}

pub fn request_target<'a>(
    endpoint: &'a BackendEndpoint,
    host_policy: &'a UpstreamHostPolicy,
    forwarded_header_policy: &'a ForwardedHeaderPolicy,
) -> RequestBuildTarget<'a> {
    RequestBuildTarget {
        endpoint,
        policies: RequestBuildPolicies {
            host_policy,
            forwarded_header_policy,
        },
    }
}

pub fn parse_backend_endpoint(backend: &str) -> Result<BackendEndpoint, BridgeError> {
    BackendEndpoint::parse(backend).map_err(|_| BridgeError::InvalidUri)
}

pub fn bridge_headers(headers: &HeaderMap) -> Vec<Header> {
    headers
        .iter()
        .map(|(name, value)| Header::new(name.as_str().as_bytes(), value.as_bytes()))
        .collect()
}

pub fn request_input<'a>(
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> RequestBuildInput<'a, BoxBody<Bytes, Infallible>> {
    RequestBuildInput {
        method,
        path,
        authority: meta.authority,
        headers,
        body: Empty::<Bytes>::new().boxed(),
        content_length: meta.content_length,
        body_mode: RequestBuildInput::<BoxBody<Bytes, Infallible>>::body_mode_for_length(
            meta.content_length,
        ),
        trace: RequestTraceContext {
            request_id: meta.request_id,
            traceparent: meta.traceparent,
        },
        forwarded: RequestForwardedContext {
            client_addr: meta.client_addr,
        },
    }
}

pub fn request_input_with_body_mode<'a>(
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
    body_mode: RequestBodyMode,
) -> RequestBuildInput<'a, BoxBody<Bytes, Infallible>> {
    let mut input = request_input(method, path, headers, meta);
    input.body_mode = body_mode;
    input
}

pub fn build_h1_and_h2_requests<'a>(
    endpoint: &'a BackendEndpoint,
    host_policy: &'a UpstreamHostPolicy,
    forwarded_header_policy: &'a ForwardedHeaderPolicy,
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> Result<CanonicalBridgeRequestPair, BridgeError> {
    let h1 = build_h1_request(
        request_target(endpoint, host_policy, forwarded_header_policy),
        request_input(method, path, headers, meta),
    )?;
    let h2 = build_h2_request_for_target(
        request_target(endpoint, host_policy, forwarded_header_policy),
        request_input(method, path, headers, meta),
    )?;
    Ok((h1, h2))
}

pub fn build_h1_request_for_backend<'a>(
    backend: &str,
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> Result<CanonicalBridgeRequest, BridgeError> {
    let endpoint = parse_backend_endpoint(backend)?;
    build_h1_request(
        request_target(
            &endpoint,
            &UpstreamHostPolicy::default(),
            &ForwardedHeaderPolicy::default(),
        ),
        request_input(method, path, headers, meta),
    )
}

pub fn build_h2_request_for_backend<'a>(
    backend: &str,
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> Result<CanonicalBridgeRequest, BridgeError> {
    let endpoint = parse_backend_endpoint(backend)?;
    build_h2_request_for_target(
        request_target(
            &endpoint,
            &UpstreamHostPolicy::default(),
            &ForwardedHeaderPolicy::default(),
        ),
        request_input(method, path, headers, meta),
    )
}

pub fn build_h1_request_with_policy<'a>(
    endpoint: &'a BackendEndpoint,
    host_policy: &'a UpstreamHostPolicy,
    forwarded_policy: &'a ForwardedHeaderPolicy,
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> Result<CanonicalBridgeRequest, BridgeError> {
    build_h1_request(
        request_target(endpoint, host_policy, forwarded_policy),
        request_input(method, path, headers, meta),
    )
}

pub fn build_h2_request_with_policy<'a>(
    endpoint: &'a BackendEndpoint,
    host_policy: &'a UpstreamHostPolicy,
    forwarded_policy: &'a ForwardedHeaderPolicy,
    method: &'a str,
    path: &'a str,
    headers: &'a [Header],
    meta: RequestInputMeta<'a>,
) -> Result<CanonicalBridgeRequest, BridgeError> {
    build_h2_request_for_target(
        request_target(endpoint, host_policy, forwarded_policy),
        request_input(method, path, headers, meta),
    )
}
