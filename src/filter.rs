//! Replay aware tracing filter.

use std::fmt::Debug;
use tracing::{
    Event, Id, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Record},
};
use tracing_subscriber::{
    Layer,
    layer::{Context, Filter},
    registry::LookupSpan,
};

#[derive(Debug)]
struct ReplayField(bool);

struct ReplayFieldVisitor(bool);

impl Visit for ReplayFieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name().eq("restate.sdk.is_replaying") {
            self.0 = value;
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn Debug) {}
}

/// Replay aware tracing filter.
///
/// Use this filter to skip tracing events in the service while replaying:
///
/// ```rust,no_run
/// use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
/// tracing_subscriber::registry()
///     .with(
///         tracing_subscriber::fmt::layer()
///             // Default Env filter to read RUST_LOG
///             .with_filter(tracing_subscriber::EnvFilter::from_default_env())
///             // Replay aware filter
///             .with_filter(restate_sdk::filter::ReplayAwareFilter)
///     )
///     .init();
/// ```
pub struct ReplayAwareFilter;

impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> Filter<S> for ReplayAwareFilter {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        true
    }

    fn event_enabled(&self, event: &Event<'_>, cx: &Context<'_, S>) -> bool {
        if let Some(scope) = cx.event_scope(event) {
            let iterator = scope.from_root();
            for span in iterator {
                if span.name() == "restate_sdk_endpoint_handle" {
                    if let Some(replay) = span.extensions().get::<ReplayField>() {
                        return !replay.0;
                    }
                }
            }
        }
        true
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if span.name() == "restate_sdk_endpoint_handle" {
                let mut visitor = ReplayFieldVisitor(false);
                attrs.record(&mut visitor);
                let mut extensions = span.extensions_mut();
                extensions.replace::<ReplayField>(ReplayField(visitor.0));
            }
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if span.name() == "restate_sdk_endpoint_handle" {
                let mut visitor = ReplayFieldVisitor(false);
                values.record(&mut visitor);
                let mut extensions = span.extensions_mut();
                extensions.replace::<ReplayField>(ReplayField(visitor.0));
            }
        }
    }
}

impl<S: Subscriber> Layer<S> for ReplayAwareFilter {}
