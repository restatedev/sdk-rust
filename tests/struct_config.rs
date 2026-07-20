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
