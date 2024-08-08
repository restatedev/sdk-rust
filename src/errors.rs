use restate_sdk_shared_core::Failure;

pub struct TerminalError;

impl From<Failure> for TerminalError {
    fn from(value: Failure) -> Self {
        todo!()
    }
}

impl From<TerminalError> for Failure {
    fn from(value: TerminalError) -> Self {
        todo!()
    }
}

pub type HandlerResult<T> = Result<T, TerminalError>;
