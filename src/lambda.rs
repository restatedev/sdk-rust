//! Lambda integration.

use std::convert::Infallible;
use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::task::{Context, Poll};

use aws_lambda_events::encodings::Base64Data;
use bytes::{Bytes, BytesMut};
use futures::future::{BoxFuture, Either};
use futures::{ready, FutureExt, Stream};
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use lambda_runtime::Service;
use lambda_runtime::{FunctionResponse, LambdaEvent};
use restate_sdk_shared_core::Header;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::endpoint::{self, InputReceiver};
use crate::endpoint::{Endpoint, OutputSender};

#[allow(clippy::declare_interior_mutable_const)]
const X_RESTATE_SERVER: HeaderName = HeaderName::from_static("x-restate-server");
const X_RESTATE_SERVER_VALUE: HeaderValue =
    HeaderValue::from_static(concat!("restate-sdk-rust/", env!("CARGO_PKG_VERSION")));

/// Represents an incoming request from AWS Lambda when using Lambda Function URLs.
///
/// This struct is used to deserialize the JSON payload from Lambda.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LambdaRequest {
    /// The HTTP method of the request.
    // #[serde(with = "http_method")]
    #[serde(with = "http_serde::method")]
    pub http_method: Method,
    /// The path of the request.
    #[serde(default)]
    pub path: String,
    /// The headers of the request.
    #[serde(with = "http_serde::header_map", default)]
    pub headers: HeaderMap,
    /// Whether the request body is Base64 encoded.
    pub is_base64_encoded: bool,
    /// The request body, if any.
    pub body: Option<Base64Data>,
}

/// Represents a response to be sent back to AWS Lambda.
///
/// This struct is serialized to JSON to form the response payload for Lambda.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LambdaResponse {
    /// The HTTP status code.
    pub status_code: u16,
    /// An optional status description.
    #[serde(default)]
    pub status_description: Option<String>,
    /// The response headers.
    #[serde(with = "http_serde::header_map", default)]
    pub headers: HeaderMap,
    /// The optional response body, Base64 encoded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Base64Data>,
    /// Whether the response body is Base64 encoded. This should generally be `true`
    /// when a body is present.
    #[serde(default)]
    pub is_base64_encoded: bool,
}

impl LambdaResponse {
    fn builder() -> LambdaResponseBuilder {
        LambdaResponseBuilder {
            status_code: 200,
            status_description: None,
            headers: HeaderMap::default(),
            body: None,
        }
    }
}

struct LambdaResponseBuilder {
    status_code: u16,
    status_description: Option<String>,
    headers: HeaderMap,
    body: Option<Base64Data>,
}

impl LambdaResponseBuilder {
    pub fn status_code(mut self, status_code: u16) -> Self {
        self.status_code = status_code;
        self.status_description = http::StatusCode::from_u16(status_code)
            .map(|s| s.to_string())
            .ok();
        self
    }

    pub fn header(mut self, key: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(key, value.into());
        self
    }

    pub fn body(mut self, body: Bytes) -> Self {
        self.body = Some(Base64Data(body.into()));
        self
    }

    pub fn build(self) -> LambdaResponse {
        LambdaResponse {
            status_code: self.status_code,
            status_description: self.status_description,
            headers: self.headers,
            body: self.body,
            is_base64_encoded: true,
        }
    }
}

impl From<endpoint::Error> for LambdaResponse {
    fn from(err: endpoint::Error) -> Self {
        LambdaResponse::builder()
            .status_code(err.status_code())
            .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE)
            .header(CONTENT_TYPE, "text/plain".parse().unwrap())
            .body(Bytes::from(err.to_string()))
            .build()
    }
}

/// A [`Stream`] that is immediately closed.
///
/// This is used as a placeholder body for buffered responses in the Lambda integration,
/// where the entire response is sent at once and no streaming body is needed.
#[doc(hidden)]
pub struct ClosedStream;
impl Stream for ClosedStream {
    type Item = Result<Bytes, Infallible>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

/// Wraps an [`Endpoint`] to implement the `lambda_runtime::Service` trait for AWS Lambda.
///
/// This adapter allows a Restate endpoint to be deployed as an AWS Lambda function.
/// It handles the conversion between Lambda's request/response format and the
/// internal representation used by the SDK.
#[derive(Clone)]
pub struct LambdaEndpoint(Endpoint);

impl LambdaEndpoint {
    pub(crate) fn new(endpoint: Endpoint) -> Self {
        Self(endpoint)
    }

    /// Runs the Lambda service.
    ///
    /// This function starts the `lambda_runtime` and begins processing incoming
    /// Lambda events, passing them to the wrapped [`Endpoint`].
    pub fn run(self) -> impl Future<Output = Result<(), lambda_runtime::Error>> {
        lambda_runtime::run(self)
    }
}

impl Service<LambdaEvent<LambdaRequest>> for LambdaEndpoint {
    type Response = FunctionResponse<LambdaResponse, ClosedStream>;
    type Error = Infallible;
    type Future = Either<
        Ready<Result<FunctionResponse<LambdaResponse, ClosedStream>, Infallible>>,
        InvocationResponseFuture,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: LambdaEvent<LambdaRequest>) -> Self::Future {
        let (request, _context) = req.into_parts();
        info!("Received request {}", request.path);

        let endpoint_response = match self.0.resolve(&request.path, request.headers) {
            Ok(res) => res,
            Err(err) => {
                debug!("Error when trying to handle incoming request: {err}");
                return ready(Ok(FunctionResponse::BufferedResponse(err.into()))).left_future();
            }
        };

        match endpoint_response {
            endpoint::Response::ReplyNow {
                status_code,
                headers,
                body,
            } => ready(Ok(response_builder_from_response_parts(
                status_code,
                headers,
                body,
            )))
            .left_future(),
            endpoint::Response::BidiStream {
                status_code,
                headers,
                handler,
            } => {
                let input_receiver = InputReceiver::from_bytes(
                    request.body.expect("required request body").0.into(),
                );

                let (output_tx, output_rx) = mpsc::unbounded_channel();
                let output_sender = OutputSender::from_channel(output_tx);

                let handler_fut = Box::pin(handler.handle(input_receiver, output_sender));

                InvocationResponseFuture {
                    status_code,
                    headers: Some(headers),
                    fut: Some(handler_fut),
                    output_rx,
                    buffer: Some(BytesMut::new()),
                }
                .right_future()
            }
        }
    }
}

fn response_builder_from_response_parts(
    status_code: u16,
    extra_headers: Vec<Header>,
    body: Bytes,
) -> FunctionResponse<LambdaResponse, ClosedStream> {
    let mut builder = LambdaResponse::builder()
        .status_code(status_code)
        .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE);

    for header in extra_headers {
        // todo: optimize this
        builder = builder.header(
            header.key.parse().expect("Invalid header name"),
            header.value.parse().expect("Invalid header value"),
        );
    }

    FunctionResponse::BufferedResponse(builder.body(body).build())
}

/// A [`Future`] that drives a bidirectional streaming handler for a Lambda invocation.
///
/// This future manages the lifecycle of a single invocation. It spawns the handler
/// future, receives messages from the handler's output stream, and aggregates them
/// into a single response body. When the handler completes, it sends the final
/// response back to Lambda.
pub struct InvocationResponseFuture {
    status_code: u16,
    headers: Option<Vec<Header>>,
    fut: Option<BoxFuture<'static, Result<(), endpoint::Error>>>,
    output_rx: mpsc::UnboundedReceiver<Bytes>,
    buffer: Option<BytesMut>,
}

impl Future for InvocationResponseFuture {
    type Output = Result<FunctionResponse<LambdaResponse, ClosedStream>, Infallible>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
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

        while let Some(out) = ready!(self.output_rx.poll_recv(cx)) {
            self.buffer.as_mut().map(|buffer| buffer.extend(out));
        }

        // end of output
        let response = response_builder_from_response_parts(
            self.status_code,
            self.headers.take().unwrap_or_default(),
            self.buffer.take().unwrap_or_default().freeze(),
        );

        Poll::Ready(Ok(response))
    }
}
