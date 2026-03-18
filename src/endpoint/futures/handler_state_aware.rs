use crate::endpoint::{ContextInternal, Error, ErrorInner};
use futures::{FutureExt, ready};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::warn;

pin_project! {
    /// Future that will stop polling when handler is suspended/failed
    pub struct HandlerStateAwareFuture<F> where F: Future {
        #[pin]
        state: HandlerStateAwareFutureState<F>,
        handler_state_rx: oneshot::Receiver<Error>,
        handler_context: ContextInternal,
    }
}

impl<F> HandlerStateAwareFuture<F>
where
    F: Future,
{
    pub fn new(
        handler_context: ContextInternal,
        handler_state_rx: oneshot::Receiver<Error>,
        fut: F,
    ) -> HandlerStateAwareFuture<F> {
        HandlerStateAwareFuture {
            state: HandlerStateAwareFutureState::Running { fut },
            handler_state_rx,
            handler_context,
        }
    }
}

pin_project! {
    #[project = HandlerStateAwareFutureStateProject]
    enum HandlerStateAwareFutureState<F> where F: Future {
        Running { #[pin] fut: F },
        Draining {
            output: Option<Result<F::Output, Error>>,
            #[pin] sleep: tokio::time::Sleep
        },
    }
}

impl<F> Future for HandlerStateAwareFuture<F>
where
    F: Future,
{
    type Output = Result<F::Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match this.state.as_mut().project() {
                HandlerStateAwareFutureStateProject::Running { fut } => {
                    match this.handler_state_rx.try_recv() {
                        Ok(e) => {
                            warn!(
                                rpc.system = "restate",
                                rpc.service = %this.handler_context.service_name(),
                                rpc.method = %this.handler_context.handler_name(),
                                "Error while processing handler {e:#}"
                            );
                            this.handler_context.consume_to_end();
                            this.state.set(HandlerStateAwareFutureState::Draining {
                                output: Some(Err(e)),
                                sleep: tokio::time::sleep(Duration::from_secs(60)),
                            });
                        }
                        Err(oneshot::error::TryRecvError::Empty) => match fut.poll(cx) {
                            Poll::Ready(output) => {
                                this.handler_context.consume_to_end();
                                this.state.set(HandlerStateAwareFutureState::Draining {
                                    output: Some(Ok(output)),
                                    sleep: tokio::time::sleep(Duration::from_secs(60)),
                                });
                                continue;
                            }
                            Poll::Pending => return Poll::Pending,
                        },
                        Err(oneshot::error::TryRecvError::Closed) => {
                            panic!(
                                "This is unexpected, this future is still being polled although the sender side was dropped. This should not be possible, because the sender is dropped when this future returns Poll:ready()."
                            )
                        }
                    }
                }
                HandlerStateAwareFutureStateProject::Draining {
                    output,
                    ref mut sleep,
                } => match this.handler_context.drain_input(cx) {
                    Poll::Ready(Ok(_)) => {
                        return Poll::Ready(output.take().expect("Future polled after completion"));
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Pending => {
                        ready!(sleep.poll_unpin(cx));
                        return Poll::Ready(Err(ErrorInner::DrainError {
                            err: "Timed out draining input stream after 60s".into(),
                        }
                        .into()));
                    }
                },
            }
        }
    }
}
