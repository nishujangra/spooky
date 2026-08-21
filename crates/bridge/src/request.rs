//! Canonical upstream request-building surface.
//!
//! This module owns request-shaping inputs, host/forwarded-header policy
//! application, and the stable entrypoints callers should use. Protocol-specific
//! H1/H2 encoding details are delegated to internal builder modules.

use std::{convert::Infallible, net::SocketAddr};

use bytes::Bytes;
use http::{HeaderName, HeaderValue};
use http_body_util::combinators::BoxBody;
use impulse_config::{
    backend_endpoint::BackendEndpoint,
    config::{ForwardedHeaderPolicy, UpstreamHostPolicy},
};
use impulse_errors::BridgeError;

use crate::{
    forwarded::{ForwardedHeaderChains, ForwardedHeaderValues, build_forwarded_header_values},
    h3_to_h1, h3_to_h2,
    headers::{connection_header_tokens, should_strip_request_header},
    host::resolve_upstream_host_value,
};

pub fn build_h1_request(
    target: RequestBuildTarget<'_>,
    input: RequestBuildInput<'_, BoxBody<Bytes, Infallible>>,
) -> Result<http::Request<BoxBody<Bytes, Infallible>>, BridgeError> {
    h3_to_h1::build_h1_request(target, input)
}

pub fn build_h2_request_for_target(
    target: RequestBuildTarget<'_>,
    input: RequestBuildInput<'_, BoxBody<Bytes, Infallible>>,
) -> Result<http::Request<BoxBody<Bytes, Infallible>>, BridgeError> {
    h3_to_h2::build_h2_request_for_target(target, input)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestBodyMode {
    Empty,
    KnownLength,
    Streaming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestTraceContext<'a> {
    pub request_id: u64,
    pub traceparent: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestForwardedContext {
    pub client_addr: SocketAddr,
}

#[derive(Clone, Copy, Debug)]
pub struct RequestBuildPolicies<'a> {
    pub host_policy: &'a UpstreamHostPolicy,
    pub forwarded_header_policy: &'a ForwardedHeaderPolicy,
}

#[derive(Debug)]
pub struct RequestBuildTarget<'a> {
    pub endpoint: &'a BackendEndpoint,
    pub policies: RequestBuildPolicies<'a>,
}

pub struct RequestBuildInput<'a, B = BoxBody<Bytes, Infallible>> {
    pub method: &'a str,
    pub path: &'a str,
    pub authority: Option<&'a str>,
    pub headers: &'a [quiche::h3::Header],
    pub body: B,
    pub content_length: Option<usize>,
    pub body_mode: RequestBodyMode,
    pub trace: RequestTraceContext<'a>,
    pub forwarded: RequestForwardedContext,
}

#[derive(Debug)]
pub(crate) struct RequestHeaderPolicyInput<'a> {
    pub(crate) target: RequestBuildTarget<'a>,
    pub(crate) authority: Option<&'a str>,
    pub(crate) headers: &'a [quiche::h3::Header],
    pub(crate) preserve_upgrade: bool,
    pub(crate) forwarded: RequestForwardedContext,
}

#[derive(Debug)]
pub(crate) struct ResolvedRequestHeaderPolicy {
    pub(crate) passthrough_headers: Vec<(HeaderName, HeaderValue)>,
    pub(crate) host_value: String,
    pub(crate) forwarded_values: ForwardedHeaderValues,
}

pub(crate) struct RequestHeaderAssembly<'a> {
    pub(crate) resolved_headers: ResolvedRequestHeaderPolicy,
    pub(crate) trace: RequestTraceContext<'a>,
    pub(crate) content_length: Option<usize>,
    pub(crate) include_content_length: bool,
    pub(crate) include_host_header: bool,
    pub(crate) add_te_trailers: bool,
}

impl<'a, B> RequestBuildInput<'a, B> {
    pub fn body_mode_for_length(content_length: Option<usize>) -> RequestBodyMode {
        match content_length {
            Some(0) => RequestBodyMode::Empty,
            Some(_) => RequestBodyMode::KnownLength,
            None => RequestBodyMode::Streaming,
        }
    }
}

pub(crate) fn apply_request_header_policies(
    input: RequestHeaderPolicyInput<'_>,
) -> Result<ResolvedRequestHeaderPolicy, BridgeError> {
    use quiche::h3::NameValue;

    let RequestHeaderPolicyInput {
        target,
        authority,
        headers,
        preserve_upgrade,
        forwarded,
    } = input;
    let RequestBuildTarget { endpoint, policies } = target;
    let connection_tokens = connection_header_tokens(headers);
    let mut passthrough_headers = Vec::new();
    let mut host_from_headers: Option<String> = None;
    let mut forwarded_from_headers: Vec<Vec<u8>> = Vec::new();
    let mut x_forwarded_for_from_headers: Vec<Vec<u8>> = Vec::new();
    let mut x_forwarded_proto_from_headers: Vec<Vec<u8>> = Vec::new();
    let mut x_forwarded_host_from_headers: Vec<Vec<u8>> = Vec::new();

    for header in headers {
        let name = header.name();
        if name.starts_with(b":") {
            continue;
        }
        if name.eq_ignore_ascii_case(b"forwarded") {
            forwarded_from_headers.push(header.value().to_vec());
            continue;
        }
        if name.eq_ignore_ascii_case(b"x-forwarded-for") {
            x_forwarded_for_from_headers.push(header.value().to_vec());
            continue;
        }
        if name.eq_ignore_ascii_case(b"x-forwarded-proto") {
            x_forwarded_proto_from_headers.push(header.value().to_vec());
            continue;
        }
        if name.eq_ignore_ascii_case(b"x-forwarded-host") {
            x_forwarded_host_from_headers.push(header.value().to_vec());
            continue;
        }

        let header_name = HeaderName::from_bytes(name).map_err(|_| BridgeError::InvalidHeader)?;
        if should_strip_request_header(&header_name, &connection_tokens, preserve_upgrade) {
            continue;
        }

        let header_value =
            HeaderValue::from_bytes(header.value()).map_err(|_| BridgeError::InvalidHeader)?;
        if header_name == http::header::HOST {
            host_from_headers = header_value.to_str().ok().map(str::to_string);
            continue;
        }
        passthrough_headers.push((header_name, header_value));
    }

    let host_value = resolve_upstream_host_value(
        endpoint,
        policies.host_policy,
        authority,
        host_from_headers.as_deref(),
    )?
    .to_string();
    let forwarded_values = build_forwarded_header_values(
        policies.forwarded_header_policy,
        ForwardedHeaderChains {
            forwarded: &forwarded_from_headers,
            x_forwarded_for: &x_forwarded_for_from_headers,
            x_forwarded_proto: &x_forwarded_proto_from_headers,
            x_forwarded_host: &x_forwarded_host_from_headers,
        },
        forwarded.client_addr.ip(),
        &host_value,
    )?;

    Ok(ResolvedRequestHeaderPolicy {
        passthrough_headers,
        host_value,
        forwarded_values,
    })
}

pub(crate) fn apply_request_header_assembly(
    mut builder: http::request::Builder,
    assembly: RequestHeaderAssembly<'_>,
) -> Result<http::request::Builder, BridgeError> {
    let RequestHeaderAssembly {
        resolved_headers,
        trace,
        content_length,
        include_content_length,
        include_host_header,
        add_te_trailers,
    } = assembly;

    for (header_name, header_value) in resolved_headers.passthrough_headers {
        builder = builder.header(header_name, header_value);
    }

    if include_host_header {
        builder = builder.header(http::header::HOST, resolved_headers.host_value.as_str());
    }

    if include_content_length
        && let Some(len) = content_length
        && len > 0
    {
        builder = builder.header(http::header::CONTENT_LENGTH, len);
    }

    let has_request_id = builder
        .headers_ref()
        .is_some_and(|h| h.contains_key("x-request-id"));
    if !has_request_id {
        builder = builder.header(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_str(&trace.request_id.to_string())
                .map_err(|_| BridgeError::InvalidHeader)?,
        );
    }

    let has_traceparent = builder
        .headers_ref()
        .is_some_and(|h| h.contains_key("traceparent"));
    if !has_traceparent && let Some(traceparent) = trace.traceparent {
        builder = builder.header(
            HeaderName::from_static("traceparent"),
            HeaderValue::from_str(traceparent).map_err(|_| BridgeError::InvalidHeader)?,
        );
    }

    if let Some(value) = resolved_headers.forwarded_values.forwarded {
        builder = builder.header(HeaderName::from_static("forwarded"), value);
    }
    if let Some(value) = resolved_headers.forwarded_values.x_forwarded_for {
        builder = builder.header(HeaderName::from_static("x-forwarded-for"), value);
    }
    if let Some(value) = resolved_headers.forwarded_values.x_forwarded_proto {
        builder = builder.header(HeaderName::from_static("x-forwarded-proto"), value);
    }
    if let Some(value) = resolved_headers.forwarded_values.x_forwarded_host {
        builder = builder.header(HeaderName::from_static("x-forwarded-host"), value);
    }

    if add_te_trailers {
        builder = builder.header(http::header::TE, "trailers");
    }

    Ok(builder)
}
