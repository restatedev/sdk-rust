use crate::endpoint;
use crate::endpoint::{Endpoint, InputReceiver, OutputSender};
use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use http_body_util::{BodyExt, Either, Full};
use hyper::body::{Body, Frame, Incoming};
use hyper::header::CONTENT_TYPE;
use hyper::http::response;
use hyper::server::conn::http2;
use hyper::service::Service;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use restate_sdk_shared_core::ResponseHead;
use std::convert::Infallible;
use std::future::{ready, Future, Ready};
use std::net::SocketAddr;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{info, warn};

pub struct HyperServer {
    endpoint: Endpoint,
}

impl From<Endpoint> for HyperServer {
    fn from(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl HyperServer {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub async fn listen_and_serve(self, addr: SocketAddr) {
        let listener = TcpListener::bind(addr).await.expect("listener can bind");
        self.serve(listener).await;
    }

    pub async fn serve(self, listener: TcpListener) {
        self.serve_with_cancel(listener, tokio::signal::ctrl_c().map(|_| ()))
            .await;
    }

    pub async fn serve_with_cancel(self, listener: TcpListener, cancel_signal_future: impl Future) {
        let endpoint = HyperEndpoint(self.endpoint);
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();

        // when this signal completes, start shutdown
        let mut signal = std::pin::pin!(cancel_signal_future);

        info!("Starting listening on {}", listener.local_addr().unwrap());

        // Our server accept loop
        loop {
            tokio::select! {
                Ok((stream, remote)) = listener.accept() => {
                    let endpoint = endpoint.clone();

                    let conn = http2::Builder::new(TokioExecutor::default())
                        .serve_connection(TokioIo::new(stream), endpoint);

                    let fut = graceful.watch(conn);

                    tokio::spawn(async move {
                        if let Err(e) = fut.await {
                            warn!("Error serving connection {remote}: {:?}", e);
                        }
                    });
                },
                _ = &mut signal => {
                    info!("Shutting down");
                    // stop the accept loop
                    break;
                }
            }
        }

        // Wait graceful shutdown
        tokio::select! {
            _ = graceful.shutdown() => {},
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                warn!("Timed out waiting for all connections to close");
            }
        }
    }
}

#[derive(Clone)]
struct HyperEndpoint(Endpoint);

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
            } => {
                return ready(Ok(response_builder_from_response_head(response_head)
                    .body(Either::Left(Full::new(body)))
                    .expect("Headers should be valid")))
            }
            endpoint::Response::BidiStream {
                response_head,
                handler,
            } => {
                let input_receiver =
                    InputReceiver::from_stream(body.into_data_stream().map_err(|e| e.into()));

                let (output_tx, output_rx) = mpsc::unbounded_channel();
                let output_sender = OutputSender::from_channel(output_tx);

                let handler_fut = Box::pin(handler.handle(input_receiver, output_sender));

                return ready(Ok(response_builder_from_response_head(response_head)
                    .body(Either::Right(BidiStreamRunner {
                        fut: Some(handler_fut),
                        output_rx,
                        end_stream: false,
                    }))
                    .expect("Headers should be valid")));
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

// TODO use pin_project
struct BidiStreamRunner {
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
