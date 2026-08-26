pub(super) fn sanitize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!(".../{name}"))
        .unwrap_or_else(|| "<path>".to_string())
}

pub(super) fn sanitize_jwks_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }

    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let Some((scheme, remainder)) = without_query.split_once("://") else {
        return sanitize_path(without_query);
    };

    let sanitized_path = remainder
        .split_once('/')
        .map(|(_, path)| path)
        .and_then(|path| path.split('/').rfind(|segment| !segment.is_empty()))
        .map(|segment| format!("/{}", segment))
        .unwrap_or_default();

    format!("{scheme}://...{sanitized_path}")
}

pub(super) fn option_is_present(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}
