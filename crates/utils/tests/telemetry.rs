use impulse_utils::telemetry::{
    endpoint::{OtlpEndpointSource, resolve_otlp_endpoint},
    init::DEFAULT_OTLP_ENDPOINT,
};

#[test]
fn config_endpoint_overrides_environment() {
    let (endpoint, source) =
        resolve_otlp_endpoint(Some(" http://cfg-collector:4317 "), |key| match key {
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" => Some("http://env-traces:4317".to_string()),
            "OTEL_EXPORTER_OTLP_ENDPOINT" => Some("http://env-generic:4317".to_string()),
            _ => None,
        });

    assert_eq!(endpoint, "http://cfg-collector:4317");
    assert_eq!(source, OtlpEndpointSource::Config);
}

#[test]
fn traces_environment_overrides_generic_environment() {
    let (endpoint, source) = resolve_otlp_endpoint(None, |key| match key {
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" => Some("http://env-traces:4317".to_string()),
        "OTEL_EXPORTER_OTLP_ENDPOINT" => Some("http://env-generic:4317".to_string()),
        _ => None,
    });

    assert_eq!(endpoint, "http://env-traces:4317");
    assert_eq!(source, OtlpEndpointSource::EnvTraces);
}

#[test]
fn generic_environment_is_used_when_traces_endpoint_is_absent() {
    let (endpoint, source) = resolve_otlp_endpoint(None, |key| match key {
        "OTEL_EXPORTER_OTLP_ENDPOINT" => Some(" http://env-generic:4317 ".to_string()),
        _ => None,
    });

    assert_eq!(endpoint, "http://env-generic:4317");
    assert_eq!(source, OtlpEndpointSource::EnvGeneric);
}

#[test]
fn default_endpoint_is_used_when_config_and_environment_are_absent() {
    let (endpoint, source) = resolve_otlp_endpoint(None, |_| None);

    assert_eq!(endpoint, DEFAULT_OTLP_ENDPOINT);
    assert_eq!(source, OtlpEndpointSource::Default);
}
