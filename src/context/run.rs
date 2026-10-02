use crate::context::DurableFuture;
use crate::endpoint::ContextInternal;
use crate::endpoint::context::RunFutureImpl;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::errors::HandlerResult;
use crate::errors::TerminalError;
use crate::serde::{Deserialize, Serialize};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

/// Run closure trait
pub trait RunClosure {
    type Output: Deserialize + Serialize + 'static;
    type Fut: Future<Output = HandlerResult<Self::Output>>;

    fn run(self) -> Self::Fut;
}

impl<F, O, Fut> RunClosure for F
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = HandlerResult<O>>,
    O: Deserialize + Serialize + 'static,
{
    type Output = O;
    type Fut = Fut;

    fn run(self) -> Self::Fut {
        self()
    }
}

/// Future created using [`ContextSideEffects::run`](super::ContextSideEffects::run).
pub trait RunFuture<O>: Future<Output = O> {
    /// Provide a custom retry policy for this `run` operation.
    ///
    /// If unspecified, the `run` will be retried using the [Restate invoker retry policy](https://docs.restate.dev/operate/configuration/server),
    /// which by default retries indefinitely.
    fn retry_policy(self, retry_policy: RunRetryPolicy) -> Self;

    /// Define a name for this `run` operation.
    ///
    /// This is used mainly for observability.
    fn name(self, name: impl Into<String>) -> Self;
}

pin_project! {
    /// Configurable journaled action returned by [`ContextSideEffects::run`](super::ContextSideEffects::run).
    ///
    /// Await directly for a sequential action, or call [`Run::start`] to register an owned
    /// action before awaiting it alongside other operations.
    pub struct Run<R: RunClosure> {
        #[pin]
        inner: InterceptErrorFuture<RunFutureImpl<R, R::Output, R::Fut>>,
    }
}

impl<R> Run<R>
where
    R: RunClosure + Send,
    R::Fut: Send,
{
    pub(crate) fn new(ctx: ContextInternal, inner: RunFutureImpl<R, R::Output, R::Fut>) -> Self {
        Self {
            inner: InterceptErrorFuture::new(ctx, inner),
        }
    }

    /// Register this action now, returning a durable future for its result.
    ///
    /// Configure the name and retry policy before calling `start`. Register all concurrent
    /// actions in deterministic order before awaiting any of them, so replay observes the
    /// same command order even when their closures complete in a different order.
    ///
    /// The invocation owns the closure after registration. Dropping the result future
    /// does not cancel the action; it can continue while another durable operation is
    /// awaited. Closures are dropped when the invocation finishes, suspends, or fails.
    /// Captured values and the closure's future must therefore be owned (`'static`).
    /// Sequential actions that borrow local values can still be awaited directly.
    ///
    /// Cancellation fails existing pending result futures with code 409 and drops their
    /// pending closures. New context operations can still be used for cleanup.
    pub fn start(
        self,
    ) -> impl DurableFuture<Output = Result<R::Output, TerminalError>> + Send + 'static
    where
        R: 'static,
        R::Fut: 'static,
    {
        let (ctx, inner) = self.inner.into_parts();
        inner.start(ctx)
    }
}

impl<R> Future for Run<R>
where
    R: RunClosure + Send,
    R::Fut: Send,
{
    type Output = Result<R::Output, TerminalError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

impl<R> RunFuture<Result<R::Output, TerminalError>> for Run<R>
where
    R: RunClosure + Send,
    R::Fut: Send,
{
    fn retry_policy(mut self, policy: RunRetryPolicy) -> Self {
        self.inner = self.inner.retry_policy(policy);
        self
    }

    fn name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.name(name);
        self
    }
}

/// This struct represents the policy to execute retries for run closures.
#[derive(Debug, Clone)]
pub struct RunRetryPolicy {
    pub(crate) initial_delay: Duration,
    pub(crate) factor: f32,
    pub(crate) max_delay: Option<Duration>,
    pub(crate) max_attempts: Option<u32>,
    pub(crate) max_duration: Option<Duration>,
}

impl Default for RunRetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            factor: 2.0,
            max_delay: Some(Duration::from_secs(2)),
            max_attempts: None,
            max_duration: Some(Duration::from_secs(50)),
        }
    }
}

impl RunRetryPolicy {
    /// Create a new retry policy.
    pub fn new() -> Self {
        Self {
            initial_delay: Duration::from_millis(100),
            factor: 1.0,
            max_delay: None,
            max_attempts: None,
            max_duration: None,
        }
    }

    /// Initial retry delay for the first retry attempt.
    pub fn initial_delay(mut self, initial_interval: Duration) -> Self {
        self.initial_delay = initial_interval;
        self
    }

    /// Exponentiation factor to use when computing the next retry delay.
    pub fn exponentiation_factor(mut self, factor: f32) -> Self {
        self.factor = factor;
        self
    }

    /// Maximum delay between retries.
    pub fn max_delay(mut self, max_interval: Duration) -> Self {
        self.max_delay = Some(max_interval);
        self
    }

    /// Gives up retrying when either at least the given number of attempts, including the initial attempt,
    /// is reached, or `max_duration` (if set) is reached first.
    ///
    /// **Note:** The number of actual retries may be higher than the provided value.
    /// This is due to the nature of the run operation, which executes the closure on the service and sends the result afterward to Restate.
    ///
    /// Infinite retries if this field and `max_duration` are unset.
    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = Some(max_attempts);
        self
    }

    /// Gives up retrying when either the retry loop lasted at least for this given max duration,
    /// or `max_attempts` (if set) is reached first.
    ///
    /// **Note:** The real retry loop duration may be higher than the given duration.
    /// This is due to the nature of the run operation, which executes the closure on the service and sends the result afterward to Restate.
    ///
    /// Infinite retries if this field and `max_attempts` are unset.
    pub fn max_duration(mut self, max_duration: Duration) -> Self {
        self.max_duration = Some(max_duration);
        self
    }
}
