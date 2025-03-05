//! Replay aware tracing filter
//!
//! Use this filter to skip tracing events in the service/workflow while replaying.
//!
//! Example:
//! ```rust,no_run
//! use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
//! let replay_filter = restate_sdk::filter::ReplayAwareFilter;
//! tracing_subscriber::registry()
//!   .with(tracing_subscriber::fmt::layer().with_filter(replay_filter))
//!   .init();
//! ```
use std::fmt::Debug;
use tracing::{
    field::{Field, Visit},
    span::{Attributes, Record},
    Event, Id, Metadata, Subscriber,
};
use tracing_subscriber::{
    layer::{Context, Filter},
    registry::LookupSpan,
    Layer,
};

#[derive(Debug)]
struct ReplayField(bool);

struct ReplayFieldVisitor(bool);

impl Visit for ReplayFieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name().eq("replaying") {
            self.0 = value;
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn Debug) {}
}

pub struct ReplayAwareFilter;

impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> Filter<S> for ReplayAwareFilter {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        true
    }

    fn event_enabled(&self, event: &Event<'_>, cx: &Context<'_, S>) -> bool {
        if let Some(scope) = cx.event_scope(event) {
            if let Some(span) = scope.from_root().next() {
                let extensions = span.extensions();
                if let Some(replay) = extensions.get::<ReplayField>() {
                    return !replay.0;
                }
            }
            true
        } else {
            true
        }
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut visitor = ReplayFieldVisitor(false);
            attrs.record(&mut visitor);
            let mut extensions = span.extensions_mut();
            extensions.insert::<ReplayField>(ReplayField(visitor.0));
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut visitor = ReplayFieldVisitor(false);
            values.record(&mut visitor);
            let mut extensions = span.extensions_mut();
            extensions.replace::<ReplayField>(ReplayField(visitor.0));
        }
    }
}

impl<S: Subscriber> Layer<S> for ReplayAwareFilter {}
