use crate::endpoint;
use crate::endpoint::{Endpoint, InputReceiver, OutputSender};
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use http::header::CONTENT_TYPE;
use http::{response, Request, Response};
use http_body_util::{BodyExt, Either, Full};
use hyper::body::{Body, Frame, Incoming};
use hyper::service::Service;
use restate_sdk_shared_core::ResponseHead;
use std::convert::Infallible;
use std::future::{ready, Ready};
use std::ops::Deref;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Clone)]
pub struct HyperEndpoint(Endpoint);

impl HyperEndpoint {
    pub fn new(endpoint: Endpoint) -> Self {
        Self(endpoint)
    }
}

impl Service<Request<Incoming>> for HyperEndpoint {
    type Response = Response<Either<Full<Bytes>, BidiStreamRunner>>;
    type Error = endpoint::Error;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let endpoint_response = match self.0.resolve(parts.uri.path(), parts.headers) {
            Ok(res) => res,
            Err(err) => {
                // TODO log this
                return ready(Ok(Response::builder()
                    .status(err.status_code())
                    .header(CONTENT_TYPE, "text/plain")
                    .body(Either::Left(Full::new(Bytes::from(err.to_string()))))
                    .expect("Headers should be valid")));
            }
        };

        match endpoint_response {
            endpoint::Response::ReplyNow {
                response_head,
                body,
            } => ready(Ok(response_builder_from_response_head(response_head)
                .body(Either::Left(Full::new(body)))
                .expect("Headers should be valid"))),
            endpoint::Response::BidiStream {
                response_head,
                handler,
            } => {
                let input_receiver =
                    InputReceiver::from_stream(body.into_data_stream().map_err(|e| e.into()));

                let (output_tx, output_rx) = mpsc::unbounded_channel();
                let output_sender = OutputSender::from_channel(output_tx);

                let handler_fut = Box::pin(handler.handle(input_receiver, output_sender));

                ready(Ok(response_builder_from_response_head(response_head)
                    .body(Either::Right(BidiStreamRunner {
                        fut: Some(handler_fut),
                        output_rx,
                        end_stream: false,
                    }))
                    .expect("Headers should be valid")))
            }
        }
    }
}

fn response_builder_from_response_head(response_head: ResponseHead) -> response::Builder {
    let mut response_builder = Response::builder().status(response_head.status_code);

    for header in response_head.headers {
        response_builder = response_builder.header(header.key.deref(), header.value.deref());
    }

    response_builder
}

pub struct BidiStreamRunner {
    fut: Option<BoxFuture<'static, Result<(), endpoint::Error>>>,
    output_rx: mpsc::UnboundedReceiver<Bytes>,
    end_stream: bool,
}

impl Body for BidiStreamRunner {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // First try to consume the runner future
        if let Some(mut fut) = self.fut.take() {
            match fut.poll_unpin(cx) {
                Poll::Ready(res) => {
                    if let Err(e) = res {
                        warn!("Handler failure: {e:?}")
                    }
                    self.output_rx.close();
                }
                Poll::Pending => {
                    self.fut = Some(fut);
                }
            }
        }

        if let Some(out) = ready!(self.output_rx.poll_recv(cx)) {
            Poll::Ready(Some(Ok(Frame::data(out))))
        } else {
            self.end_stream = true;
            Poll::Ready(None)
        }
    }

    fn is_end_stream(&self) -> bool {
        self.end_stream
    }
}
