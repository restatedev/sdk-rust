pub fn inject_opentelemetry_headers(headers: &mut http::HeaderMap) {
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry_http::HeaderInjector;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    TraceContextPropagator::new().inject_context(
        &tracing::Span::current().context(),
        &mut HeaderInjector(headers),
    );
}
