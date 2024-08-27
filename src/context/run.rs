use crate::errors::HandlerResult;
use crate::serde::{Deserialize, Serialize};
use std::future::Future;
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

/// Future created using [`super::ContextSideEffects::run`].
pub trait RunFuture<O>: Future<Output = O> {
    /// Provide a custom retry policy for this `run` operation.
    ///
    /// If unspecified, the `run` will be retried using the [Restate invoker retry policy](https://docs.restate.dev/operate/configuration/server),
    /// which by default retries indefinitely.
    fn with_retry_policy(self, retry_policy: RunRetryPolicy) -> Self;

    /// Define a name for this `run` operation.
    ///
    /// This is used mainly for observability.
    fn named(self, name: impl Into<String>) -> Self;
}

/// This struct represents the policy to execute retries for run closures.
#[derive(Debug, Clone)]
pub struct RunRetryPolicy {
    pub(crate) initial_interval: Duration,
    pub(crate) factor: f32,
    pub(crate) max_interval: Option<Duration>,
    pub(crate) max_attempts: Option<u32>,
    pub(crate) max_duration: Option<Duration>,
}

impl Default for RunRetryPolicy {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_millis(100),
            factor: 2.0,
            max_interval: Some(Duration::from_secs(2)),
            max_attempts: None,
            max_duration: Some(Duration::from_secs(50)),
        }
    }
}

impl RunRetryPolicy {
    /// Create a new retry policy.
    pub fn new() -> Self {
        Self {
            initial_interval: Duration::from_millis(100),
            factor: 1.0,
            max_interval: None,
            max_attempts: None,
            max_duration: None,
        }
    }

    /// Initial interval for the first retry attempt.
    pub fn with_initial_interval(mut self, initial_interval: Duration) -> Self {
        self.initial_interval = initial_interval;
        self
    }

    /// Maximum interval between retries.
    pub fn with_factor(mut self, factor: f32) -> Self {
        self.factor = factor;
        self
    }

    /// Maximum interval between retries.
    pub fn with_max_interval(mut self, max_interval: Duration) -> Self {
        self.max_interval = Some(max_interval);
        self
    }

    /// Gives up retrying when either at least the given number of attempts is reached,
    /// or `max_duration` (if set) is reached first.
    ///
    /// **Note:** The number of actual retries may be higher than the provided value.
    /// This is due to the nature of the run operation, which executes the closure on the service and sends the result afterward to Restate.
    ///
    /// Infinite retries if this field and `max_duration` are unset.
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
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
    pub fn with_max_duration(mut self, max_duration: Duration) -> Self {
        self.max_duration = Some(max_duration);
        self
    }
}
