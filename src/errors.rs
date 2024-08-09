use restate_sdk_shared_core::Failure;

pub struct TerminalError {
    pub code: u16,
    pub message: String,
}

impl TerminalError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            code: 500,
            message: message.into(),
        }
    }
}

impl From<Failure> for TerminalError {
    fn from(value: Failure) -> Self {
        Self {
            code: value.code,
            message: value.message,
        }
    }
}

impl From<TerminalError> for Failure {
    fn from(value: TerminalError) -> Self {
        Self {
            code: value.code,
            message: value.message,
        }
    }
}

pub type HandlerResult<T> = Result<T, TerminalError>;
