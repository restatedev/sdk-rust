//! ## Running on Lambda
//!
//! The Restate Rust SDK supports running services on AWS Lambda using Lambda Function URLs. This allows you to deploy your Restate services as serverless functions.
//!
//! ### Setup
//!
//! First, enable the `lambda` feature in your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! restate-sdk = { version = "0.1", features = ["lambda"] }
//! tokio = { version = "1", features = ["full"] }
//! ```
//!
//! ### Basic Lambda Service
//!
//! Here's how to create a simple Lambda service:
//!
//! ```rust,no_run
//! #![cfg(feature = "lambda")]
//!
//! use restate_sdk::prelude::*;
//!
//! #[restate_sdk::service]
//! trait Greeter {
//!     async fn greet(name: String) -> HandlerResult<String>;
//! }
//!
//! struct GreeterImpl;
//!
//! impl Greeter for GreeterImpl {
//!     async fn greet(&self, _: Context<'_>, name: String) -> HandlerResult<String> {
//!         Ok(format!("Greetings {name}"))
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     // To enable logging/tracing
//!     // check https://docs.aws.amazon.com/lambda/latest/dg/rust-logging.html#rust-logging-tracing
//!
//!     // Build and run the Lambda endpoint
//!     LambdaEndpoint::run(
//!         Endpoint::builder()
//!             .bind(GreeterImpl.serve())
//!             .build(),
//!     )
//!     .await
//!     .unwrap();
//! }
//! ```
//!
//! ### Deployment
//!
//! 1. Install `cargo-lambda`
//!    ```bash
//!    cargo install cargo-lambda
//!    ```
//! 2. Build your Lambda function:
//!
//!    ```bash
//!    cargo lambda build --release --arm64 --output-format zip
//!    ```
//!
//! 3. Create a Lambda function with the following configuration:
//!
//!    - **Runtime**: Amazon Linux 2023
//!    - **Architecture**: arm64
//!
//! 4. Upload your `zip` file to the Lambda function.

use std::future::Future;

use aws_lambda_events::encodings::Base64Data;
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Uri};
use http_body_util::{BodyExt, Full};
use lambda_runtime::service_fn;
use lambda_runtime::tower::ServiceExt;
use lambda_runtime::LambdaEvent;
use serde::{Deserialize, Serialize};

use crate::endpoint::{Endpoint, HandleOptions, ProtocolMode};

/// Represents an incoming request from AWS Lambda when using Lambda Function URLs.
///
/// This struct is used to deserialize the JSON payload from Lambda.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct LambdaRequest {
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
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LambdaResponse {
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

/// Wraps an [`Endpoint`] to implement the `lambda_runtime::Service` trait for AWS Lambda.
///
/// This adapter allows a Restate endpoint to be deployed as an AWS Lambda function.
/// It handles the conversion between Lambda's request/response format and the
/// internal representation used by the SDK.
#[derive(Clone)]
pub struct LambdaEndpoint;

impl LambdaEndpoint {
    /// Runs the Lambda service.
    ///
    /// This function starts the `lambda_runtime` and begins processing incoming
    /// Lambda events, passing them to the wrapped [`Endpoint`].
    pub fn run(endpoint: Endpoint) -> impl Future<Output = Result<(), lambda_runtime::Error>> {
        let svc = service_fn(handle);
        let svc = svc.map_request(move |req| {
            let endpoint = endpoint.clone();
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

async fn handle(req: LambdaEventWithEndpoint) -> Result<LambdaResponse, lambda_runtime::Error> {
    let (request, _) = req.inner.into_parts();

    let mut http_request = Request::builder()
        .method(request.http_method)
        .uri(request.path)
        .body(request.body.map(|b| Full::from(b.0)).unwrap_or_default())
        .expect("to build");

    http_request.headers_mut().extend(request.headers);

    let response = req.endpoint.handle_with_options(
        http_request,
        HandleOptions {
            protocol_mode: ProtocolMode::RequestResponse,
        },
    );

    let (parts, body) = response.into_parts();

    let body = body.collect().await?.to_bytes();

    let mut builder = LambdaResponse::builder().status_code(parts.status.as_u16());
    builder.headers.extend(parts.headers);

    Ok(builder.body(body).build())
}
