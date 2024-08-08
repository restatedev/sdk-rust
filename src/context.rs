use crate::endpoint::HandlerContext;
use crate::errors::TerminalError;
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

// TODO figure out later a constructor
pub struct Context<'a>(pub HandlerContext, pub    PhantomData<&'a ()>,);

impl<'a> Context<'a> {
    pub fn sleep(&self, duration: Duration) -> impl Future<Output = Result<(), TerminalError>> + 'a {
        self.0.sleep(duration)
    }

    pub fn get(&self, key: &str) -> impl Future<Output = Result<Option<Vec<u8>>, TerminalError>> + 'a {
        self.0.get_state(&key)
    }
}
