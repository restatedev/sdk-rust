use restate_sdk_shared_core::Failure;
use std::error::Error as StdError;
use std::fmt;
use thiserror::__private::AsDynError;

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
            HandlerErrorInner::Retryable(e) => Some(e.as_dyn_error()),
            HandlerErrorInner::Terminal(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub struct HandlerError(pub(crate) HandlerErrorInner);

impl HandlerError {
    #[cfg(feature = "anyhow")]
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        Self(HandlerErrorInner::Retryable(err.into()))
    }
}

impl<E: StdError + Send + Sync + 'static> From<E> for HandlerError {
    fn from(value: E) -> Self {
        Self(HandlerErrorInner::Retryable(Box::new(value)))
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

#[derive(Debug, Clone)]
pub struct TerminalError(pub(crate) TerminalErrorInner);

impl TerminalError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::new_with_code(500, message)
    }

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

pub type HandlerResult<T> = Result<T, HandlerError>;
