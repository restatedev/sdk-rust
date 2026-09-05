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

pub fn set_opentelemetry_parent(span: &tracing::Span, headers: &[restate_sdk_shared_core::Header]) {
    use opentelemetry::propagation::{Extractor, TextMapPropagator};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    struct InvocationHeaders<'a>(&'a [restate_sdk_shared_core::Header]);

    impl Extractor for InvocationHeaders<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0
                .iter()
                .find(|header| header.key == key)
                .map(|header| header.value.as_ref())
        }

        fn keys(&self) -> Vec<&str> {
            self.0.iter().map(|header| header.key.as_ref()).collect()
        }
    }

    let parent = TraceContextPropagator::new().extract(&InvocationHeaders(headers));
    let _ = span.set_parent(parent);
}
