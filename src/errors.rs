//! # Error Handling
//!
//! Restate handles retries for failed invocations.
//! By default, Restate does infinite retries with an exponential backoff strategy.
//!
//! For failures for which you do not want retries, but instead want the invocation to end and the error message
//! to be propagated back to the caller, you can throw a **terminal error**.
//!
//! You can throw a terminal exception with an optional HTTP status code and a message anywhere in your handler, as follows:
//!
//! ```
//! # use restate_sdk::prelude::*;
//! # async fn handle() -> Result<(), HandlerError> {
//! Err(TerminalError::new("This is a terminal error").into())
//! # }
//! ```
//!
//! You can catch terminal exceptions. For example, you can catch the terminal exception that comes out of a [call to another service][service_communication#request-response-calls], and build your control flow around it.
use restate_sdk_shared_core::Failure;
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub(crate) enum HandlerErrorInner {
    Retryable(Box<dyn StdError + Send + Sync + 'static>),
    Terminal(TerminalErrorInner),
}

impl fmt::Display for HandlerErrorInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HandlerErrorInner::Retryable(e) => {
                write!(f, "Retryable error: {}", e)
            }
            HandlerErrorInner::Terminal(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl StdError for HandlerErrorInner {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            HandlerErrorInner::Retryable(e) => Some(e.as_ref()),
            HandlerErrorInner::Terminal(e) => Some(e),
        }
    }
}

/// This error can contain either a [`TerminalError`], or any other Rust's [`StdError`].
/// For the latter, the error is considered "retryable", and the execution will be retried.
#[derive(Debug)]
pub struct HandlerError(pub(crate) HandlerErrorInner);

impl<E: Into<Box<dyn StdError + Send + Sync + 'static>>> From<E> for HandlerError {
    fn from(value: E) -> Self {
        Self(HandlerErrorInner::Retryable(value.into()))
    }
}

impl From<TerminalError> for HandlerError {
    fn from(value: TerminalError) -> Self {
        Self(HandlerErrorInner::Terminal(value.0))
    }
}

// Took from anyhow
impl AsRef<dyn StdError + Send + Sync> for HandlerError {
    fn as_ref(&self) -> &(dyn StdError + Send + Sync + 'static) {
        &self.0
    }
}

impl AsRef<dyn StdError> for HandlerError {
    fn as_ref(&self) -> &(dyn StdError + 'static) {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalErrorInner {
    code: u16,
    message: String,
}

impl fmt::Display for TerminalErrorInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Terminal error [{}]: {}", self.code, self.message)
    }
}

impl StdError for TerminalErrorInner {}

/// Error representing the result of an operation recorded in the journal.
///
/// When returned inside a [`crate::context::ContextSideEffects::run`] closure, or in a handler, it completes the operation with a failure value.
#[derive(Debug, Clone)]
pub struct TerminalError(pub(crate) TerminalErrorInner);

impl TerminalError {
    /// Create a new [`TerminalError`].
    pub fn new(message: impl Into<String>) -> Self {
        Self::new_with_code(500, message)
    }

    /// Create a new [`TerminalError`] with a status code.
    pub fn new_with_code(code: u16, message: impl Into<String>) -> Self {
        Self(TerminalErrorInner {
            code,
            message: message.into(),
        })
    }

    pub fn code(&self) -> u16 {
        self.0.code
    }

    pub fn message(&self) -> &str {
        &self.0.message
    }

    pub fn from_error<E: StdError>(e: E) -> Self {
        Self::new(e.to_string())
    }
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl AsRef<dyn StdError + Send + Sync> for TerminalError {
    fn as_ref(&self) -> &(dyn StdError + Send + Sync + 'static) {
        &self.0
    }
}

impl AsRef<dyn StdError> for TerminalError {
    fn as_ref(&self) -> &(dyn StdError + 'static) {
        &self.0
    }
}

impl From<Failure> for TerminalError {
    fn from(value: Failure) -> Self {
        Self(TerminalErrorInner {
            code: value.code,
            message: value.message,
        })
    }
}

impl From<TerminalError> for Failure {
    fn from(value: TerminalError) -> Self {
        Self {
            code: value.0.code,
            message: value.0.message,
        }
    }
}

/// Result type for a Restate handler.
pub type HandlerResult<T> = Result<T, HandlerError>;
