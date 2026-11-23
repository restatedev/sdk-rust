//! Tests that the configuration attributes on the struct-based service API land in the generated
//! discovery: service-wide defaults, per-handler overrides, the `invocation_retry_policy(..)` group,
//! and `///` doc-comment capture.

use restate_sdk::discovery::{Handler, RetryPolicyOnMaxAttempts, Service};
use restate_sdk::prelude::*;
use restate_sdk::service::Discoverable;

fn handler<'a>(disc: &'a Service, name: &str) -> &'a Handler {
    disc.handlers
        .iter()
        .find(|h| h.name.as_str() == name)
        .unwrap_or_else(|| panic!("handler {name} not found"))
}

/// A fully configured service.
struct ConfiguredService;

#[service(
    name = "Configured",
    inactivity_timeout = "30s",
    abort_timeout = "1m",
    journal_retention = "1 day",
    idempotency_retention = "2 days",
    lazy_state = true,
    ingress_private = false,
    preemptable = false,
    invocation_retry_policy(
        initial_interval = "100ms",
        max_interval = "10s",
        factor = 2.0,
        max_attempts = 5,
        on_max_attempts = "pause",
    )
)]
impl ConfiguredService {
    /// Documented, overriding handler.
    #[handler(
        inactivity_timeout = "5s",
        preemptable = true,
        invocation_retry_policy(max_attempts = 3, on_max_attempts = "kill")
    )]
    async fn configured(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }

    /// A handler with no config overrides.
    #[handler]
    async fn plain(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn service_level_options_are_applied() {
    let disc = <ConfiguredService as Discoverable>::discover();

    assert_eq!(disc.name.to_string(), "Configured");
    assert_eq!(disc.inactivity_timeout, Some(30_000));
    assert_eq!(disc.abort_timeout, Some(60_000));
    assert_eq!(disc.journal_retention, Some(86_400_000));
    assert_eq!(disc.idempotency_retention, Some(172_800_000));
    assert_eq!(disc.enable_lazy_state, Some(true));
    assert_eq!(disc.ingress_private, Some(false));
    assert_eq!(disc.preemptable, Some(false));

    assert_eq!(disc.retry_policy_initial_interval, Some(100));
    assert_eq!(disc.retry_policy_max_interval, Some(10_000));
    assert_eq!(disc.retry_policy_exponentiation_factor, Some(2.0));
    assert_eq!(disc.retry_policy_max_attempts, Some(5));
    assert!(matches!(
        disc.retry_policy_on_max_attempts,
        Some(RetryPolicyOnMaxAttempts::Pause)
    ));
}

#[test]
fn handler_level_options_override_and_capture_docs() {
    let disc = <ConfiguredService as Discoverable>::discover();

    let configured = handler(&disc, "configured");
    assert_eq!(configured.inactivity_timeout, Some(5_000));
    assert_eq!(configured.preemptable, Some(true));
    assert_eq!(configured.retry_policy_max_attempts, Some(3));
    assert!(matches!(
        configured.retry_policy_on_max_attempts,
        Some(RetryPolicyOnMaxAttempts::Kill)
    ));
    assert_eq!(
        configured.documentation.as_deref(),
        Some("Documented, overriding handler.")
    );

    let plain = handler(&disc, "plain");
    assert_eq!(plain.inactivity_timeout, None);
    assert_eq!(plain.preemptable, None);
    assert_eq!(plain.retry_policy_max_attempts, None);
    assert_eq!(
        plain.documentation.as_deref(),
        Some("A handler with no config overrides.")
    );
}

/// A workflow that sets the completion retention at the workflow level; it lands on the `run`
/// handler in the discovery.
struct ConfiguredWorkflow;

#[workflow(name = "ConfiguredWf", workflow_completion_retention = "3 days")]
impl ConfiguredWorkflow {
    #[handler]
    async fn run(&self, _ctx: WorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }

    #[handler]
    async fn status(&self, _ctx: SharedWorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn workflow_completion_retention_lands_on_run_handler() {
    let disc = <ConfiguredWorkflow as Discoverable>::discover();
    // Applied to the main (non-shared) `run` handler...
    assert_eq!(
        handler(&disc, "run").workflow_completion_retention,
        Some(259_200_000)
    );
    // ...but not to shared handlers.
    assert_eq!(handler(&disc, "status").workflow_completion_retention, None);
}

/// A virtual object with a bare-flag `lazy_state` and no `= true`.
struct FlagObject;

#[object(name = "FlagObject", lazy_state)]
impl FlagObject {
    #[handler(ingress_private)]
    async fn touch(&self, _ctx: ObjectContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn bare_bool_flags_mean_true() {
    let disc = <FlagObject as Discoverable>::discover();
    assert_eq!(disc.enable_lazy_state, Some(true));
    assert_eq!(handler(&disc, "touch").ingress_private, Some(true));
}

/// `ServiceOptions::name` binds a service under another name: discovery reports the new name,
/// and invocations are routed by it.
#[tokio::test]
async fn service_options_name_renames_the_bound_service() {
    use http_body_util::BodyExt;
    use restate_sdk::endpoint::{Endpoint, ServiceOptions};
    use restate_sdk::service::IntoServiceDefinition;

    let endpoint = Endpoint::builder()
        .bind(
            ConfiguredService
                .into_service_definition()
                .options(ServiceOptions::new().name("Renamed")),
        )
        .build();
    let request = http::Request::builder()
        .uri("/discover")
        .header(
            http::header::ACCEPT,
            "application/vnd.restate.endpointmanifest.v4+json",
        )
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    let response = endpoint.handle(request);
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let manifest: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = manifest["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|service| service["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Renamed"]);

    let unknown = http::Request::builder()
        .method("POST")
        .uri("/invoke/Configured/plain")
        .header(
            http::header::CONTENT_TYPE,
            "application/vnd.restate.invocation.v5",
        )
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    assert_eq!(
        endpoint.handle(unknown).status(),
        404,
        "the old name is gone"
    );
}

async fn discover(endpoint: &Endpoint, accept: &str) -> (String, serde_json::Value) {
    use http_body_util::BodyExt;

    let request = http::Request::builder()
        .uri("/discover")
        .header(http::header::ACCEPT, accept)
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    let response = endpoint.handle(request);
    assert_eq!(response.status(), 200);
    let content_type = response.headers()[http::header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .to_owned();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (content_type, serde_json::from_slice(&body).unwrap())
}

/// `preemptable` is published only when explicitly set, and only from discovery protocol V6 on:
/// older versions silently drop it, since servers that old never preempt invocations.
#[tokio::test]
async fn preemptable_is_published_only_when_set_and_from_discovery_v6() {
    const V4: &str = "application/vnd.restate.endpointmanifest.v4+json";
    const V6: &str = "application/vnd.restate.endpointmanifest.v6+json";

    // `Configured` sets it through the macro attributes, `FlagObject` through the options builder.
    let endpoint = Endpoint::builder()
        .bind(ConfiguredService)
        .bind(FlagObject.into_service_definition().options(
            ServiceOptions::new().handler("touch", HandlerOptions::new().preemptable(false)),
        ))
        .build();
    let preemptable = |manifest: &serde_json::Value, service: &str, handler: Option<&str>| {
        let service = manifest["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == service)
            .unwrap();
        let target = match handler {
            None => service,
            Some(handler) => service["handlers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["name"] == handler)
                .unwrap(),
        };
        target.get("preemptable").cloned()
    };

    // V6 is negotiated when offered alongside older versions.
    let (content_type, manifest) = discover(&endpoint, &format!("{V4}, {V6}")).await;
    assert_eq!(content_type, V6);
    assert_eq!(
        preemptable(&manifest, "Configured", None),
        Some(false.into())
    );
    assert_eq!(
        preemptable(&manifest, "Configured", Some("configured")),
        Some(true.into())
    );
    assert_eq!(preemptable(&manifest, "Configured", Some("plain")), None);
    assert_eq!(preemptable(&manifest, "FlagObject", None), None);
    assert_eq!(
        preemptable(&manifest, "FlagObject", Some("touch")),
        Some(false.into())
    );

    // Below V6 the field is dropped everywhere, without failing discovery.
    let (content_type, manifest) = discover(&endpoint, V4).await;
    assert_eq!(content_type, V4);
    assert_eq!(preemptable(&manifest, "Configured", None), None);
    assert_eq!(
        preemptable(&manifest, "Configured", Some("configured")),
        None
    );
    assert_eq!(preemptable(&manifest, "FlagObject", Some("touch")), None);
}
