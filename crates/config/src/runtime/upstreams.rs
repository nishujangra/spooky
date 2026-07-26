use super::*;

type RouteMatcherKey = RuntimeRouteMatchPolicy;

impl RuntimeUpstream {
    pub(super) fn from_config(
        config: &Config,
        name: &str,
        upstream: &Upstream,
        base_policies: &RuntimePolicySet,
    ) -> Result<Self, RuntimeConfigError> {
        let effective_tls = upstream
            .tls
            .clone()
            .unwrap_or_else(|| config.upstream_tls.clone());
        let load_balancing = RuntimeLoadBalancingPolicy::normalize(&upstream.load_balancing)?;
        let route = RuntimeRouteMatchPolicy::normalize(name, &upstream.route)?;
        let policy = RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy::normalize(&upstream.auth, name)?,
            host: RuntimeHostPolicy(upstream.host_policy.clone()),
            forwarded_headers: RuntimeForwardedHeaderPolicy(upstream.forwarded_headers.clone()),
            protocol: base_policies.admission.protocol.clone(),
        };
        let runtime_upstream = Self {
            name: name.to_string(),
            load_balancing: load_balancing.clone(),
            route: route.clone(),
            policy,
            effective_tls: effective_tls.clone(),
            backend_tls_policy: RuntimeBackendTlsPolicy::from_effective_tls(&effective_tls),
            backends: upstream
                .backends
                .iter()
                .map(|backend| RuntimeBackend::normalize(name, backend))
                .collect::<Result<Vec<_>, _>>()?,
        };

        Ok(runtime_upstream)
    }

    #[cfg(test)]
    pub(crate) fn as_config_upstream(&self) -> Upstream {
        Upstream {
            load_balancing: self.load_balancing.as_config(),
            auth: self.policy.upstream_auth.as_config(),
            host_policy: self.policy.host.0.clone(),
            forwarded_headers: self.policy.forwarded_headers.0.clone(),
            tls: Some(self.effective_tls.clone()),
            route: self.route.as_config(),
            backends: self
                .backends
                .iter()
                .map(|backend| {
                    let mut config_backend = backend.backend.clone();
                    config_backend.health_check = backend
                        .health_check
                        .as_ref()
                        .map(RuntimeBackendHealthCheck::as_config);
                    config_backend
                })
                .collect(),
        }
    }
}

pub(super) fn normalize_upstreams(
    config: &Config,
    base_policies: &RuntimePolicySet,
) -> Result<HashMap<String, RuntimeUpstream>, RuntimeConfigError> {
    if config.upstream.is_empty() {
        return Err(RuntimeConfigError::ConfigInvalid(
            "no upstreams configured".to_string(),
        ));
    }

    validate_protocol_policy(&config.resilience.protocol)?;

    let mut seen_route_matchers: HashMap<RouteMatcherKey, String> = HashMap::new();
    let mut seen_backend_origins: HashMap<String, (String, String)> = HashMap::new();
    let mut normalized = HashMap::new();

    let mut ordered_upstreams: Vec<(&String, &Upstream)> = config.upstream.iter().collect();
    ordered_upstreams.sort_by_key(|(upstream_name, _)| *upstream_name);

    for (upstream_name, upstream) in ordered_upstreams {
        validate_upstream_policy(config, upstream_name, upstream)?;

        let route_key = RuntimeRouteMatchPolicy::normalize(upstream_name, &upstream.route)?;
        if let Some(existing) = seen_route_matchers.insert(route_key.clone(), upstream_name.clone())
        {
            return Err(RuntimeConfigError::DuplicateRouteAmbiguity {
                upstream: upstream_name.clone(),
                existing_upstream: existing,
                host: route_key.host.clone(),
                path_prefix: route_key.path_prefix.clone(),
                method: route_key.method.clone(),
            });
        }

        let runtime_upstream =
            RuntimeUpstream::from_config(config, upstream_name.as_str(), upstream, base_policies)?;
        let mut upstream_uses_https_backends = false;

        for backend in &runtime_upstream.backends {
            if matches!(
                backend.endpoint.transport_kind,
                RuntimeBackendTransportKind::H2
            ) {
                upstream_uses_https_backends = true;
            }

            if let Some((existing_upstream, existing_backend)) = seen_backend_origins.insert(
                backend.endpoint.origin.clone(),
                (upstream_name.clone(), backend.backend.id.clone()),
            ) {
                return Err(RuntimeConfigError::BackendAddressInvalid {
                    upstream: upstream_name.clone(),
                    backend: backend.backend.id.clone(),
                    address: backend.endpoint.origin.clone(),
                    reason: format!(
                        "conflicts with upstream '{}' backend '{}'",
                        existing_upstream, existing_backend
                    ),
                });
            }
        }

        if upstream_uses_https_backends {
            validate_runtime_upstream_tls(
                upstream_name,
                &runtime_upstream.backend_tls_policy.as_upstream_tls(),
            )?;
        }

        normalized.insert(upstream_name.clone(), runtime_upstream);
    }

    Ok(normalized)
}

impl RuntimeBackend {
    pub(super) fn normalize(
        upstream_name: &str,
        backend: &Backend,
    ) -> Result<Self, RuntimeConfigError> {
        if backend.id.trim().is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' contains an empty backend id"
            )));
        }
        if backend.address.trim().is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "backend '{}' in upstream '{}' has an empty address",
                backend.id, upstream_name
            )));
        }

        Ok(Self {
            backend: backend.clone(),
            endpoint: RuntimeBackendEndpoint::normalize(
                upstream_name,
                backend.id.as_str(),
                backend.address.as_str(),
            )?,
            health_check: backend
                .health_check
                .as_ref()
                .map(|health_check| {
                    RuntimeBackendHealthCheck::normalize(
                        upstream_name,
                        backend.id.as_str(),
                        health_check,
                    )
                })
                .transpose()?,
        })
    }
}

fn validate_protocol_policy(policy: &ProtocolPolicy) -> Result<(), RuntimeConfigError> {
    if policy.max_headers_count == 0 {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.max_headers_count must be greater than 0".to_string(),
        ));
    }
    if policy.max_headers_bytes == 0 {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.max_headers_bytes must be greater than 0".to_string(),
        ));
    }
    if policy
        .allowed_methods
        .iter()
        .any(|method| method.trim().is_empty())
    {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.allowed_methods must not contain empty values".to_string(),
        ));
    }
    if policy
        .denied_path_prefixes
        .iter()
        .any(|prefix| prefix.is_empty() || !prefix.starts_with('/'))
    {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.denied_path_prefixes must contain '/'-prefixed paths".to_string(),
        ));
    }
    if !policy.allow_connect
        && (!policy.connect_allowed_ports.is_empty()
            || !policy.connect_allowed_authorities.is_empty())
    {
        return Err(RuntimeConfigError::UnsupportedPolicyCombination(
            "resilience.protocol.connect_allowed_ports/connect_allowed_authorities require allow_connect=true"
                .to_string(),
        ));
    }
    if policy.connect_allowed_ports.contains(&0) {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.connect_allowed_ports must contain ports in range 1-65535"
                .to_string(),
        ));
    }
    if policy
        .connect_allowed_authorities
        .iter()
        .any(|authority| !is_valid_connect_authority(authority))
    {
        return Err(RuntimeConfigError::ConfigInvalid(
            "resilience.protocol.connect_allowed_authorities must contain authority-form host:port targets"
                .to_string(),
        ));
    }
    if policy.allow_0rtt && policy.early_data_safe_methods.is_empty() {
        return Err(RuntimeConfigError::UnsupportedPolicyCombination(
            "resilience.protocol.early_data_safe_methods must be non-empty when allow_0rtt=true"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_runtime_external_auth_headers(
    upstream_name: &str,
    field_prefix: &str,
    request_headers: &[crate::config::ExternalAuthRequestHeader],
    response_header_allowlist: &[String],
) -> Result<(), RuntimeConfigError> {
    let mut seen_request_headers = std::collections::HashSet::new();
    for header in request_headers {
        let header_name = header.name.trim();
        if header_name.is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.request_headers[].name must be non-empty"
            )));
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.request_headers[].name must be a valid HTTP header name"
            )));
        }
        if http::HeaderValue::from_str(header.value.as_str()).is_err() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.request_headers[].value must be a valid HTTP header value"
            )));
        }
        if !seen_request_headers.insert(header_name.to_ascii_lowercase()) {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.request_headers contains duplicate header names"
            )));
        }
    }

    let mut seen_allowed_headers = std::collections::HashSet::new();
    for header_name in response_header_allowlist {
        let header_name = header_name.trim();
        if header_name.is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.response_header_allowlist[] must be non-empty"
            )));
        }
        if http::header::HeaderName::from_bytes(header_name.as_bytes()).is_err() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.response_header_allowlist[] must be a valid HTTP header name"
            )));
        }
        if !seen_allowed_headers.insert(header_name.to_ascii_lowercase()) {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' {field_prefix}.response_header_allowlist contains duplicate header names"
            )));
        }
    }

    Ok(())
}

fn validate_upstream_policy(
    config: &Config,
    upstream_name: &str,
    upstream: &Upstream,
) -> Result<(), RuntimeConfigError> {
    match upstream.host_policy.mode {
        UpstreamHostPolicyMode::PassThrough | UpstreamHostPolicyMode::Upstream => {
            if upstream.host_policy.host.is_some() {
                return Err(RuntimeConfigError::UnsupportedPolicyCombination(format!(
                    "upstream '{upstream_name}' sets host_policy.host but mode is not rewrite"
                )));
            }
        }
        UpstreamHostPolicyMode::Rewrite => match upstream.host_policy.host.as_deref() {
            Some(host) if valid_static_host_header(host) => {}
            _ => {
                return Err(RuntimeConfigError::UnsupportedPolicyCombination(format!(
                    "upstream '{upstream_name}' requires a valid non-empty host_policy.host when mode=rewrite"
                )));
            }
        },
    }

    if let Some(path) = upstream.route.path_prefix.as_deref()
        && (path.is_empty() || !path.starts_with('/'))
    {
        return Err(RuntimeConfigError::ConfigInvalid(format!(
            "upstream '{upstream_name}' has an invalid route.path_prefix '{}'",
            path
        )));
    }

    if normalized_route_method(upstream.route.method.as_deref()).as_deref() == Some("CONNECT")
        && !config.resilience.protocol.allow_connect
    {
        return Err(RuntimeConfigError::UnsupportedPolicyCombination(format!(
            "upstream '{upstream_name}' routes CONNECT but resilience.protocol.allow_connect=false"
        )));
    }

    if let Some(api_key) = upstream.auth.api_key.as_ref() {
        if api_key.header_name.trim().is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.api_key.header_name must be non-empty"
            )));
        }
        if http::header::HeaderName::from_bytes(api_key.header_name.trim().as_bytes()).is_err() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.api_key.header_name must be a valid HTTP header name"
            )));
        }
        if api_key.keys.is_empty() || api_key.keys.iter().any(|value| value.trim().is_empty()) {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.api_key.keys must contain at least one non-empty key"
            )));
        }
        let mut seen_api_keys = std::collections::HashSet::new();
        for key in &api_key.keys {
            if !seen_api_keys.insert(key.trim().to_string()) {
                return Err(RuntimeConfigError::ConfigInvalid(format!(
                    "upstream '{upstream_name}' auth.api_key.keys contains duplicate values"
                )));
            }
        }
    }

    if let Some(external_auth) = upstream.auth.external_auth.as_ref() {
        if upstream.auth.api_key.is_some() || upstream.auth.jwt.is_some() {
            return Err(RuntimeConfigError::UnsupportedPolicyCombination(format!(
                "upstream '{upstream_name}' auth.external_auth cannot be combined with auth.api_key or auth.jwt in v1"
            )));
        }
        if !upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty() {
            return Err(RuntimeConfigError::UnsupportedPolicyCombination(format!(
                "upstream '{upstream_name}' auth.external_auth cannot be combined with auth.required_scopes or auth.required_roles in v1"
            )));
        }

        match external_auth {
            crate::config::ExternalAuth::Http {
                endpoint,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                ..
            } => {
                let valid_endpoint = endpoint
                    .trim()
                    .parse::<http::Uri>()
                    .ok()
                    .is_some_and(|uri| {
                        matches!(uri.scheme_str(), Some("http") | Some("https"))
                            && uri.authority().is_some()
                    });
                if !valid_endpoint {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.http.endpoint must be an absolute http(s) URL"
                    )));
                }
                validate_runtime_external_auth_headers(
                    upstream_name,
                    "auth.external_auth.http",
                    request_headers,
                    response_header_allowlist,
                )?;
                if *timeout_ms == 0 {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.http.timeout_ms must be greater than 0"
                    )));
                }
            }
            crate::config::ExternalAuth::Oidc {
                discovery_url,
                issuer_url,
                client_id,
                client_secret,
                audience,
                scopes,
                request_headers,
                response_header_allowlist,
                timeout_ms,
                ..
            } => {
                let has_discovery_url = discovery_url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
                let has_issuer_url = issuer_url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
                if !has_discovery_url && !has_issuer_url {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc requires discovery_url or issuer_url"
                    )));
                }
                if let Some(discovery_url) = discovery_url.as_deref() {
                    let valid_discovery_url = discovery_url
                        .trim()
                        .parse::<http::Uri>()
                        .ok()
                        .is_some_and(|uri| {
                            matches!(uri.scheme_str(), Some("http") | Some("https"))
                                && uri.authority().is_some()
                        });
                    if !discovery_url.trim().is_empty() && !valid_discovery_url {
                        return Err(RuntimeConfigError::ConfigInvalid(format!(
                            "upstream '{upstream_name}' auth.external_auth.oidc.discovery_url must be an absolute http(s) URL"
                        )));
                    }
                }
                if let Some(issuer_url) = issuer_url.as_deref() {
                    let valid_issuer_url =
                        issuer_url
                            .trim()
                            .parse::<http::Uri>()
                            .ok()
                            .is_some_and(|uri| {
                                matches!(uri.scheme_str(), Some("http") | Some("https"))
                                    && uri.authority().is_some()
                            });
                    if !issuer_url.trim().is_empty() && !valid_issuer_url {
                        return Err(RuntimeConfigError::ConfigInvalid(format!(
                            "upstream '{upstream_name}' auth.external_auth.oidc.issuer_url must be an absolute http(s) URL"
                        )));
                    }
                }
                if client_id.trim().is_empty() {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.client_id must be non-empty"
                    )));
                }
                if client_secret
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.client_secret must be non-empty when provided"
                    )));
                }
                if audience
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.audience must be non-empty when provided"
                    )));
                }
                if scopes.iter().any(|scope| scope.trim().is_empty()) {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.scopes must not contain empty values"
                    )));
                }
                validate_runtime_external_auth_headers(
                    upstream_name,
                    "auth.external_auth.oidc",
                    request_headers,
                    response_header_allowlist,
                )?;
                if *timeout_ms == 0 {
                    return Err(RuntimeConfigError::ConfigInvalid(format!(
                        "upstream '{upstream_name}' auth.external_auth.oidc.timeout_ms must be greater than 0"
                    )));
                }
            }
        }
    }

    if let Some(jwt) = upstream.auth.jwt.as_ref() {
        if jwt.secret.trim().is_empty() {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.jwt.secret must be non-empty"
            )));
        }
        if jwt
            .issuer
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.jwt.issuer must be non-empty when provided"
            )));
        }
        if jwt
            .audience
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(RuntimeConfigError::ConfigInvalid(format!(
                "upstream '{upstream_name}' auth.jwt.audience must be non-empty when provided"
            )));
        }
    }
    if upstream
        .auth
        .required_scopes
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(RuntimeConfigError::ConfigInvalid(format!(
            "upstream '{upstream_name}' auth.required_scopes must not contain empty values"
        )));
    }
    if upstream
        .auth
        .required_roles
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(RuntimeConfigError::ConfigInvalid(format!(
            "upstream '{upstream_name}' auth.required_roles must not contain empty values"
        )));
    }
    if (!upstream.auth.required_scopes.is_empty() || !upstream.auth.required_roles.is_empty())
        && upstream.auth.jwt.is_none()
    {
        return Err(RuntimeConfigError::ConfigInvalid(format!(
            "upstream '{upstream_name}' auth.required_scopes/auth.required_roles require auth.jwt"
        )));
    }

    Ok(())
}

fn validate_runtime_upstream_tls(
    upstream_name: &str,
    tls: &UpstreamTls,
) -> Result<(), RuntimeConfigError> {
    if tls
        .ca_file
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(RuntimeConfigError::TlsMaterialInvalid(format!(
            "upstream '{upstream_name}' has an empty effective upstream_tls.ca_file"
        )));
    }
    if tls
        .ca_dir
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(RuntimeConfigError::TlsMaterialInvalid(format!(
            "upstream '{upstream_name}' has an empty effective upstream_tls.ca_dir"
        )));
    }
    Ok(())
}

fn normalized_route_method(method: Option<&str>) -> Option<String> {
    method
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
}

fn valid_static_host_header(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed == value
        && !trimmed.chars().any(|ch| ch.is_ascii_whitespace())
        && !trimmed.contains('/')
        && !trimmed.contains('?')
        && !trimmed.contains('#')
        && http::HeaderValue::from_str(trimmed).is_ok()
}

pub(super) fn normalize_sni_server_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains(':')
        || trimmed.contains('*')
        || trimmed.chars().any(char::is_whitespace)
    {
        return None;
    }
    let without_trailing_dot = trimmed.trim_end_matches('.');
    if without_trailing_dot.is_empty() {
        return None;
    }
    let ascii = idna::domain_to_ascii(without_trailing_dot).ok()?;
    if ascii.parse::<IpAddr>().is_ok() {
        return None;
    }
    Some(ascii.to_ascii_lowercase())
}

fn is_valid_connect_authority(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return false;
    }

    if let Some(rest) = trimmed.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let suffix = &rest[end + 1..];
        if !suffix.starts_with(':') || suffix.len() <= 1 {
            return false;
        }
        return suffix[1..].parse::<u16>().ok().is_some_and(|port| port > 0);
    }

    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() || host.contains(':') {
        return false;
    }
    port.parse::<u16>().ok().is_some_and(|value| value > 0)
}
