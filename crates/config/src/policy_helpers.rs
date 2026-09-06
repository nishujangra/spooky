/// Returns whether the configured methods are safe for HTTP/3 early data.
pub(crate) fn early_data_methods_are_safe(methods: &[String]) -> bool {
    !methods.is_empty()
        && methods
            .iter()
            .all(|method| matches!(method.trim().to_ascii_uppercase().as_str(), "GET" | "HEAD"))
}
