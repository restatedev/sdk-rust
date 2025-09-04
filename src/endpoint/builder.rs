use crate::endpoint::{BoxedService, Endpoint, EndpointInner, Error};
use crate::service::{Discoverable, Service};
use futures::future::BoxFuture;
use restate_sdk_shared_core::{IdentityVerifier, KeyError};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Various configuration options that can be provided when binding a service
#[derive(Default, Debug, Clone)]
pub struct ServiceOptions {
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) inactivity_timeout: Option<Duration>,
    pub(crate) abort_timeout: Option<Duration>,
    pub(crate) idempotency_retention: Option<Duration>,
    pub(crate) journal_retention: Option<Duration>,
    pub(crate) enable_lazy_state: Option<bool>,
    pub(crate) ingress_private: Option<bool>,
    // Retry policy options
    pub(crate) retry_policy_initial_interval: Option<Duration>,
    pub(crate) retry_policy_exponentiation_factor: Option<f64>,
    pub(crate) retry_policy_max_interval: Option<Duration>,
    pub(crate) retry_policy_max_attempts: Option<u64>,
    pub(crate) retry_policy_on_max_attempts: Option<crate::discovery::RetryPolicyOnMaxAttempts>,
    pub(crate) handler_options: HashMap<String, HandlerOptions>,

    _priv: (),
}

impl ServiceOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// This timer guards against stalled invocations. Once it expires, Restate triggers a graceful
    /// termination by asking the invocation to suspend (which preserves intermediate progress).
    ///
    /// The abort_timeout is used to abort the invocation, in case it doesn't react to the request to
    /// suspend.
    ///
    /// This overrides the default inactivity timeout configured in the restate-server for all
    /// invocations to this service.
    pub fn inactivity_timeout(mut self, timeout: Duration) -> Self {
        self.inactivity_timeout = Some(timeout);
        self
    }

    /// This timer guards against stalled service/handler invocations that are supposed to terminate. The
    /// abort timeout is started after the inactivity_timeout has expired and the service/handler
    /// invocation has been asked to gracefully terminate. Once the timer expires, it will abort the
    /// service/handler invocation.
    ///
    /// This timer potentially *interrupts* user code. If the user code needs longer to gracefully
    /// terminate, then this value needs to be set accordingly.
    ///
    /// This overrides the default abort timeout configured in the restate-server for all invocations to
    /// this service.
    pub fn abort_timeout(mut self, timeout: Duration) -> Self {
        self.abort_timeout = Some(timeout);
        self
    }

    /// The retention duration of idempotent requests to this service.
    pub fn idempotency_retention(mut self, retention: Duration) -> Self {
        self.idempotency_retention = Some(retention);
        self
    }

    /// The journal retention. When set, this applies to all requests to all handlers of this service.
    ///
    /// In case the request has an idempotency key, the idempotency_retention caps the journal retention
    /// time.
    pub fn journal_retention(mut self, retention: Duration) -> Self {
        self.journal_retention = Some(retention);
        self
    }

    /// When set to `true`, lazy state will be enabled for all invocations to this service. This is
    /// relevant only for workflows and virtual objects.
    pub fn enable_lazy_state(mut self, enable: bool) -> Self {
        self.enable_lazy_state = Some(enable);
        self
    }

    /// When set to `true` this service, with all its handlers, cannot be invoked from the restate-server
    /// HTTP and Kafka ingress, but only from other services.
    pub fn ingress_private(mut self, private: bool) -> Self {
        self.ingress_private = Some(private);
        self
    }

    /// Custom metadata of this service definition. This metadata is shown on the Admin API when querying the service definition.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Initial delay before the first retry attempt.
    ///
    /// If unset, the server default is used.
    pub fn retry_policy_initial_interval(mut self, interval: Duration) -> Self {
        self.retry_policy_initial_interval = Some(interval);
        self
    }

    /// Exponential backoff multiplier used to compute the next retry delay.
    ///
    /// For attempt n, the next delay is roughly previousDelay * exponentiationFactor,
    /// capped by retry_policy_max_interval if set.
    pub fn retry_policy_exponentiation_factor(mut self, factor: f64) -> Self {
        self.retry_policy_exponentiation_factor = Some(factor);
        self
    }

    /// Upper bound for the computed retry delay.
    pub fn retry_policy_max_interval(mut self, interval: Duration) -> Self {
        self.retry_policy_max_interval = Some(interval);
        self
    }

    /// Maximum number of attempts before giving up retrying.
    ///
    /// The initial call counts as the first attempt; retries increment the count by 1.
    pub fn retry_policy_max_attempts(mut self, attempts: u64) -> Self {
        self.retry_policy_max_attempts = Some(attempts);
        self
    }

    /// Behavior when the configured retry_policy_max_attempts is reached: pause the invocation.
    ///
    /// The invocation enters the paused state and can be manually resumed from the CLI or UI.
    pub fn retry_policy_pause_on_max_attempts(mut self) -> Self {
        self.retry_policy_on_max_attempts = Some(crate::discovery::RetryPolicyOnMaxAttempts::Pause);
        self
    }

    /// Behavior when the configured retry_policy_max_attempts is reached: kill the invocation.
    ///
    /// The invocation will be marked as failed and will not be retried unless explicitly re-triggered.
    pub fn retry_policy_kill_on_max_attempts(mut self) -> Self {
        self.retry_policy_on_max_attempts = Some(crate::discovery::RetryPolicyOnMaxAttempts::Kill);
        self
    }

    /// Handler-specific options.
    ///
    /// *Note*: If you provide a handler name for a non-existing handler, binding the service will *panic!*.
    pub fn handler(mut self, handler_name: impl Into<String>, options: HandlerOptions) -> Self {
        self.handler_options.insert(handler_name.into(), options);
        self
    }
}

/// Various configuration options that can be provided when binding a service handler
#[derive(Default, Debug, Clone)]
pub struct HandlerOptions {
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) inactivity_timeout: Option<Duration>,
    pub(crate) abort_timeout: Option<Duration>,
    pub(crate) idempotency_retention: Option<Duration>,
    pub(crate) workflow_retention: Option<Duration>,
    pub(crate) journal_retention: Option<Duration>,
    pub(crate) ingress_private: Option<bool>,
    pub(crate) enable_lazy_state: Option<bool>,
    // Retry policy options
    pub(crate) retry_policy_initial_interval: Option<Duration>,
    pub(crate) retry_policy_exponentiation_factor: Option<f64>,
    pub(crate) retry_policy_max_interval: Option<Duration>,
    pub(crate) retry_policy_max_attempts: Option<u64>,
    pub(crate) retry_policy_on_max_attempts: Option<crate::discovery::RetryPolicyOnMaxAttempts>,

    _priv: (),
}

impl HandlerOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Custom metadata of this handler definition. This metadata is shown on the Admin API when querying the service/handler definition.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// This timer guards against stalled invocations. Once it expires, Restate triggers a graceful
    /// termination by asking the invocation to suspend (which preserves intermediate progress).
    ///
    /// The abort_timeout is used to abort the invocation, in case it doesn't react to the request to
    /// suspend.
    ///
    /// This overrides the inactivity timeout set for the service and the default set in restate-server.
    pub fn inactivity_timeout(mut self, timeout: Duration) -> Self {
        self.inactivity_timeout = Some(timeout);
        self
    }

    /// This timer guards against stalled invocations that are supposed to terminate. The abort timeout
    /// is started after the inactivity_timeout has expired and the invocation has been asked to
    /// gracefully terminate. Once the timer expires, it will abort the invocation.
    ///
    /// This timer potentially *interrupts* user code. If the user code needs longer to gracefully
    /// terminate, then this value needs to be set accordingly.
    ///
    /// This overrides the abort timeout set for the service and the default set in restate-server.
    pub fn abort_timeout(mut self, timeout: Duration) -> Self {
        self.abort_timeout = Some(timeout);
        self
    }

    /// The retention duration of idempotent requests to this service.
    pub fn idempotency_retention(mut self, retention: Duration) -> Self {
        self.idempotency_retention = Some(retention);
        self
    }

    /// The retention duration for this workflow handler.
    pub fn workflow_retention(mut self, retention: Duration) -> Self {
        self.workflow_retention = Some(retention);
        self
    }

    /// The journal retention for invocations to this handler.
    ///
    /// In case the request has an idempotency key, the idempotency_retention caps the journal retention
    /// time.
    pub fn journal_retention(mut self, retention: Duration) -> Self {
        self.journal_retention = Some(retention);
        self
    }

    /// When set to `true` this handler cannot be invoked from the restate-server HTTP and Kafka ingress,
    /// but only from other services.
    pub fn ingress_private(mut self, private: bool) -> Self {
        self.ingress_private = Some(private);
        self
    }

    /// When set to `true`, lazy state will be enabled for all invocations to this handler. This is
    /// relevant only for workflows and virtual objects.
    pub fn enable_lazy_state(mut self, enable: bool) -> Self {
        self.enable_lazy_state = Some(enable);
        self
    }

    /// Initial delay before the first retry attempt.
    ///
    /// If unset, the server default is used.
    pub fn retry_policy_initial_interval(mut self, interval: Duration) -> Self {
        self.retry_policy_initial_interval = Some(interval);
        self
    }

    /// Exponential backoff multiplier used to compute the next retry delay.
    ///
    /// For attempt n, the next delay is roughly previousDelay * exponentiationFactor,
    /// capped by retry_policy_max_interval if set.
    pub fn retry_policy_exponentiation_factor(mut self, factor: f64) -> Self {
        self.retry_policy_exponentiation_factor = Some(factor);
        self
    }

    /// Upper bound for the computed retry delay.
    pub fn retry_policy_max_interval(mut self, interval: Duration) -> Self {
        self.retry_policy_max_interval = Some(interval);
        self
    }

    /// Maximum number of attempts before giving up retrying.
    ///
    /// The initial call counts as the first attempt; retries increment the count by 1.
    pub fn retry_policy_max_attempts(mut self, attempts: u64) -> Self {
        self.retry_policy_max_attempts = Some(attempts);
        self
    }

    /// Behavior when the configured retry_policy_max_attempts is reached: pause the invocation.
    ///
    /// The invocation enters the paused state and can be manually resumed from the CLI or UI.
    pub fn retry_policy_pause_on_max_attempts(mut self) -> Self {
        self.retry_policy_on_max_attempts = Some(crate::discovery::RetryPolicyOnMaxAttempts::Pause);
        self
    }

    /// Behavior when the configured retry_policy_max_attempts is reached: kill the invocation.
    ///
    /// The invocation will be marked as failed and will not be retried unless explicitly re-triggered.
    pub fn retry_policy_kill_on_max_attempts(mut self) -> Self {
        self.retry_policy_on_max_attempts = Some(crate::discovery::RetryPolicyOnMaxAttempts::Kill);
        self
    }
}

/// Builder for [`Endpoint`]
#[derive(Default)]
pub struct Builder {
    svcs: HashMap<String, BoxedService>,
    discovery_services: Vec<crate::discovery::Service>,
    identity_verifier: IdentityVerifier,
}

impl Builder {
    /// Create a new builder for [`Endpoint`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a [`Service`] to this endpoint.
    ///
    /// When using the [`service`](macro@crate::service), [`object`](macro@crate::object) or [`workflow`](macro@crate::workflow) macros,
    /// you need to pass the result of the `serve` method.
    pub fn bind<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>>
            + Discoverable
            + Send
            + Sync
            + 'static,
    >(
        self,
        s: S,
    ) -> Self {
        self.bind_with_options(s, ServiceOptions::default())
    }

    /// Like [`bind`], but providing options
    pub fn bind_with_options<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>>
            + Discoverable
            + Send
            + Sync
            + 'static,
    >(
        mut self,
        s: S,
        service_options: ServiceOptions,
    ) -> Self {
        // Discover and apply options
        let mut service_metadata = S::discover();
        service_metadata.apply_options(service_options);

        let boxed_service = BoxedService::new(s);
        self.svcs
            .insert(service_metadata.name.to_string(), boxed_service);
        self.discovery_services.push(service_metadata);
        self
    }

    /// Add identity key, e.g. `publickeyv1_ChjENKeMvCtRnqG2mrBK1HmPKufgFUc98K8B3ononQvp`.
    pub fn identity_key(mut self, key: &str) -> Result<Self, KeyError> {
        self.identity_verifier = self.identity_verifier.with_key(key)?;
        Ok(self)
    }

    /// Build the [`Endpoint`].
    pub fn build(self) -> Endpoint {
        Endpoint(Arc::new(EndpointInner {
            svcs: self.svcs,
            discovery_services: self.discovery_services,
            identity_verifier: self.identity_verifier,
        }))
    }
}
