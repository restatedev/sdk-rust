use opentelemetry::trace::{TraceContextExt, TracerProvider};
use restate_sdk::ingress::ReqwestClient;
use restate_sdk::prelude::*;
use restate_sdk_testcontainers::TestEnvironment;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

static RETRY_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static RETRY_TRACE_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn retry_trace_ids() -> MutexGuard<'static, Vec<String>> {
    match RETRY_TRACE_IDS.lock() {
        Ok(trace_ids) => trace_ids,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct TraceService;

#[service]
impl TraceService {
    #[handler]
    async fn retry_trace_id(&self, _ctx: Context<'_>) -> HandlerResult<Json<Vec<String>>> {
        let trace_id = tracing::Span::current()
            .context()
            .span()
            .span_context()
            .trace_id()
            .to_string();
        retry_trace_ids().push(trace_id);

        if RETRY_ATTEMPTS.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(std::io::Error::other("retry once").into());
        }

        Ok(Json(retry_trace_ids().clone()))
    }
}

#[tokio::test]
async fn propagates_ingress_trace_across_retries() {
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    tracing_subscriber::registry()
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("restate-sdk-testcontainers")),
        )
        .init();

    let environment = TestEnvironment::new()
        .start(Endpoint::builder().bind(TraceService).build())
        .await
        .unwrap();
    let ingress_url = environment.ingress_url();

    let root_span = tracing::info_span!("retrying_service_ingress_call");
    let expected_trace_id = root_span
        .context()
        .span()
        .span_context()
        .trace_id()
        .to_string();
    let service = TraceServiceIngressClient::from_client(
        ReqwestClient::connect(ingress_url.parse().unwrap()).unwrap(),
    );
    let observed_trace_ids = service
        .retry_trace_id()
        .call()
        .instrument(root_span)
        .await
        .unwrap()
        .into_body()
        .unwrap()
        .into_inner();

    assert_eq!(observed_trace_ids.len(), 2);
    assert!(
        observed_trace_ids
            .iter()
            .all(|trace_id| trace_id == &expected_trace_id)
    );
}
