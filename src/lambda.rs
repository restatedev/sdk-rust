//! Lambda integration.

use std::convert::Infallible;
use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::task::{Context, Poll};

use aws_lambda_events::encodings::Base64Data;
use bytes::{Bytes, BytesMut};
use futures::future::Either;
use futures::{ready, FutureExt, Stream};
use http::header::CONTENT_TYPE;
use http::response::Parts;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Uri};
use http_body_util::Full;
use lambda_runtime::Service;
use lambda_runtime::{FunctionResponse, LambdaEvent};
use pin_project_lite::pin_project;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::endpoint::{Endpoint, Error, HandleOptions, ProtocolMode, ResponseBody};

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
    #[serde(with = "http_serde::uri")]
    pub path: Uri,
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

    fn from_message<M: ToString>(code: u16, message: M) -> Self {
        Self::builder()
            .status_code(code)
            .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE)
            .header(CONTENT_TYPE, "text/plain".parse().unwrap())
            .body(Bytes::from(message.to_string()))
            .build()
    }
}

impl From<LambdaResponse> for FunctionResponse<LambdaResponse, ClosedStream> {
    fn from(response: LambdaResponse) -> Self {
        FunctionResponse::BufferedResponse(response)
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

impl From<Error> for LambdaResponse {
    fn from(err: Error) -> Self {
        LambdaResponse::from_message(err.status_code(), err.to_string())
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
    pub fn new(endpoint: Endpoint) -> Self {
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
        let (request, _) = req.into_parts();

        let mut http_request = Request::builder()
            .method(request.http_method)
            .uri(request.path)
            .body(request.body.map(|b| Full::from(b.0)).unwrap_or_default())
            .expect("to build");

        http_request.headers_mut().extend(request.headers);

        let response = match self.0.handle_with_options(
            http_request,
            HandleOptions {
                protocol_mode: ProtocolMode::RequestResponse,
            },
        ) {
            Ok(res) => res,
            Err(err) => {
                debug!("Error when trying to handle incoming request: {err}");
                return ready(Ok(FunctionResponse::BufferedResponse(err.into()))).left_future();
            }
        };

        InvocationResponseFuture::new(response).right_future()
    }
}

pin_project! {

    /// A [`Future`] that drives a bidirectional streaming handler for a Lambda invocation.
    ///
    /// This future manages the lifecycle of a single invocation. It spawns the handler
    /// future, receives messages from the handler's output stream, and aggregates them
    /// into a single response body. When the handler completes, it sends the final
    /// response back to Lambda.

    pub struct InvocationResponseFuture {
        parts: Parts,
        #[pin]
        body: ResponseBody,
        buffer: Option<BytesMut>,
    }
}

impl InvocationResponseFuture {
    fn new(response: http::Response<ResponseBody>) -> Self {
        let (parts, body) = response.into_parts();
        Self {
            parts,
            body,
            buffer: Some(BytesMut::new()),
        }
    }
}

impl Future for InvocationResponseFuture {
    type Output = Result<FunctionResponse<LambdaResponse, ClosedStream>, Infallible>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use http_body::Body;

        loop {
            let frame = match ready!(self.as_mut().project().body.poll_frame(cx)) {
                Some(frame) => frame,
                None => {
                    // end of output
                    let mut builder = LambdaResponse::builder()
                        .status_code(self.parts.status.as_u16())
                        .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE);

                    // todo: optimize this
                    builder.headers.extend(self.parts.headers.clone());

                    return Poll::Ready(Ok(FunctionResponse::BufferedResponse(
                        builder
                            .body(self.buffer.take().unwrap_or_default().freeze())
                            .build(),
                    )));
                }
            };

            let frame = match frame {
                Ok(frame) if frame.is_data() => frame,
                Ok(_) => {
                    return Poll::Ready(Ok(LambdaResponse::from_message(
                        500,
                        "unexpected frame type",
                    )
                    .into()));
                }
                Err(e) => {
                    return Poll::Ready(Ok(LambdaResponse::from_message(500, e).into()));
                }
            };

            self.buffer
                .as_mut()
                .map(|buffer| buffer.extend(frame.into_data().expect("data frame")));
        }
    }
}
