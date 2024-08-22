use crate::errors::HandlerResult;
use crate::serde::{Deserialize, Serialize};
use std::future::Future;

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
