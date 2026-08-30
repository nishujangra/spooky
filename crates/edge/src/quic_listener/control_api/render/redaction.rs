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

pub(super) fn option_is_present(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}
