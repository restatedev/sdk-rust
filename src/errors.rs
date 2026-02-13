//! # Error Handling
//!
//! Restate handles retries for failed invocations.
//! By default, Restate does infinite retries with an exponential backoff strategy.
//!
//! For failures for which you do not want retries, but instead want the invocation to end and the error message
//! to be propagated back to the caller, you can return a [`TerminalError`].
//!
//! You can return a [`TerminalError`] with an optional HTTP status code and a message anywhere in your handler, as follows:
//!
//! ```rust,no_run
//! # use restate_sdk::prelude::*;
//! # async fn handle() -> Result<(), HandlerError> {
//! Err(TerminalError::new("This is a terminal error").into())
//! # }
//! ```
//!
//! You can catch terminal exceptions. For example, you can catch the terminal exception that comes out of a [call to another service][crate::context::ContextClient], and build your control flow around it.
//!
//! ## Converting Errors to Terminal Errors
//!
//! The [`TerminalErrorExt`] trait provides a convenient way to convert any `Result` error
//! into a terminal error using the `.terminal()` or `.terminal_with_code()` methods:
//!
//! ```rust,no_run
//! # use restate_sdk::prelude::*;
//! # use restate_sdk::TerminalErrorExt;
//! # async fn handle() -> Result<(), HandlerError> {
//! let parsed: i32 = "not a number".parse().terminal()?;
//! # Ok(())
//! # }
//! ```
use restate_sdk_shared_core::TerminalFailure;
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

impl From<TerminalFailure> for TerminalError {
    fn from(value: TerminalFailure) -> Self {
        Self(TerminalErrorInner {
            code: value.code,
            message: value.message,
        })
    }
}

impl From<TerminalError> for TerminalFailure {
    fn from(value: TerminalError) -> Self {
        Self {
            code: value.0.code,
            message: value.0.message,
        }
    }
}

/// Result type for a Restate handler.
pub type HandlerResult<T> = Result<T, HandlerError>;

/// Extension trait for converting any `Result` error into a [`TerminalError`].
///
/// This trait provides a convenient way to convert errors from fallible operations
/// into terminal errors that will not be retried by Restate.
///
/// # Example
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
/// use restate_sdk::TerminalErrorExt;
///
/// async fn handle() -> Result<(), HandlerError> {
///     let parsed: i32 = "not a number".parse().terminal()?;
///     Ok(())
/// }
/// ```
///
/// You can also specify a custom HTTP status code:
///
/// ```rust,no_run
/// use restate_sdk::prelude::*;
/// use restate_sdk::TerminalErrorExt;
///
/// async fn handle() -> Result<(), HandlerError> {
///     let parsed: i32 = "not a number".parse().terminal_with_code(400)?;
///     Ok(())
/// }
/// ```
pub trait TerminalErrorExt<T, E> {
    /// Convert the error into a [`TerminalError`] with the default status code (500).
    fn terminal(self) -> Result<T, HandlerError>;
    /// Convert the error into a [`TerminalError`] with a custom status code.
    fn terminal_with_code(self, code: u16) -> Result<T, HandlerError>;
}

impl<T, E> TerminalErrorExt<T, E> for Result<T, E>
where
    E: std::fmt::Display + Send + Sync + 'static,
{
    fn terminal(self) -> Result<T, HandlerError> {
        self.map_err(|err| TerminalError::new(err.to_string()).into())
    }

    fn terminal_with_code(self, code: u16) -> Result<T, HandlerError> {
        self.map_err(|err| TerminalError::new_with_code(code, err.to_string()).into())
    }
}
