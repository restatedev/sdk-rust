//! Lambda integration.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use aws_lambda_events::encodings::Base64Data;
use bytes::Bytes;
use futures::Stream;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Uri};
use http_body_util::{BodyExt, Full};
use lambda_runtime::service_fn;
use lambda_runtime::tower::ServiceExt;
use lambda_runtime::{FunctionResponse, LambdaEvent};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::endpoint::{Endpoint, Error, HandleOptions, ProtocolMode};

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
        let svc = service_fn(handle);
        let svc = svc.map_request(move |req| {
            let endpoint = self.0.clone();
            LambdaEventWithEndpoint {
                inner: req,
                endpoint,
            }
        });

        lambda_runtime::run(svc)
    }
}

struct LambdaEventWithEndpoint {
    inner: LambdaEvent<LambdaRequest>,
    endpoint: Endpoint,
}

async fn handle(req: LambdaEventWithEndpoint) -> Result<LambdaResponse, Infallible> {
    let (request, _) = req.inner.into_parts();

    let mut http_request = Request::builder()
        .method(request.http_method)
        .uri(request.path)
        .body(request.body.map(|b| Full::from(b.0)).unwrap_or_default())
        .expect("to build");

    http_request.headers_mut().extend(request.headers);

    let response = match req.endpoint.handle_with_options(
        http_request,
        HandleOptions {
            protocol_mode: ProtocolMode::RequestResponse,
        },
    ) {
        Ok(res) => res,
        Err(err) => {
            debug!("Error when trying to handle incoming request: {err}");
            return Ok(err.into());
        }
    };

    let (parts, body) = response.into_parts();
    // collect the response
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(err) => {
            debug!("Error when trying to collect response body: {err}");
            return Ok(LambdaResponse::from_message(500, err));
        }
    };

    let mut builder = LambdaResponse::builder()
        .status_code(parts.status.as_u16())
        .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE);

    builder.headers.extend(parts.headers);

    Ok(builder.body(body).build())
}
