//! Ingress client for invoking Restate handlers over HTTP.
//!
//! This module provides a typed HTTP client surface for calling Restate
//! services, virtual objects, and workflows from outside handler execution.
//!
//! ## When to use ingress
//!
//! Use this module when you want to invoke Restate handlers from:
//!
//! - external services,
//! - jobs/cron processes,
//! - tests/integration harnesses,
//! - CLIs or admin tools.
//!
//! If you are calling another handler from within a currently executing
//! handler, prefer the context client APIs in [`crate::context`].
//!
//! ## Core types
//!
//! - [`Client`]: shared HTTP client for building typed service/object/workflow clients.
//! - [`ServerUrl`]: validated base URL used to construct request targets.
//! - [`AuthToken`]: bearer token wrapper with redacted debug output.
//! - [`Request`]: fluent request builder returned by generated client methods.
//! - [`RequestError`]: errors for request build/send/status/JSON handling.
//!
//! ## Typical flow
//!
//! 1. Construct a [`ServerUrl`] and optional [`AuthToken`].
//! 2. Create a [`Client`].
//! 3. Obtain a generated client with:
//!    - `client.service_client::<MyServiceClient>()`
//!    - `client.object_client::<MyObjectClient>(key)`
//!    - `client.workflow_client::<MyWorkflowClient>(key)`
//! 4. Invoke a method, optionally configure request metadata, then call:
//!    - `.header(...)` / `.headers(...)`
//!    - `.idempotency_key(...)`
//!    - `.timeout(...)`
//!    - `.delay(...)`
//!    - `.call(&executor).await`
//!
//! ## Typed client example
//!
//! ```no_run
//! # use restate_sdk::ingress::{AuthToken, Client, ServerUrl, RequestError};
//! # use restate_sdk::ingress::executor::{Executor, HttpRequest, HttpResponse};
//! # use std::future::Future;
//! # use std::pin::Pin;
//! #[restate_sdk::service]
//! trait Greeter {
//!     async fn greet(name: String) -> restate_sdk::errors::HandlerResult<String>;
//! }
//! # struct DemoExecutor;
//! # impl Executor for DemoExecutor {
//! #     fn execute<'a>(&'a self, _request: HttpRequest) -> Pin<Box<dyn Future<Output = Result<HttpResponse, RequestError>> + Send + 'a>> {
//! #         Box::pin(async { Err(RequestError::Request("demo executor".to_string())) })
//! #     }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let executor = DemoExecutor;
//! let server_url: ServerUrl = "https://api.example.com".try_into()?;
//! let token = AuthToken::new("token".to_string().into())?;
//! let client = Client::new(server_url, Some(token));
//!
//! # async fn invoke(client: Client) -> Result<(), Box<dyn std::error::Error>> {
//! let executor = DemoExecutor;
//! let response: String = client
//!     .service_client::<GreeterClient>()
//!     .greet("Ada".to_string())
//!     .idempotency_key("greet-ada")
//!     .call(&executor)
//!     .await?;
//!
//! # let _ = response;
//! # Ok(())
//! # }
//! # let _ = invoke(client);
//! # Ok(())
//! # }
//! ```
//!
//! ## Reqwest executor
//!
//! Enable feature `reqwest-client` to use `reqwest::Client` as an ingress executor.
//!
//! ## Notes
//!
//! - The ingress client is cheap to clone and can be shared across threads.
//! - Non-success HTTP responses are surfaced as [`RequestError::Status`] with
//!   both status code and response body.
//! - Successful responses are JSON-decoded into the handler return type.
//!
use http::Uri;
use http::header::AUTHORIZATION;
use http::header::{HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue};
use http::uri::Authority;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use std::marker::PhantomData;
use std::time::Duration;
use thiserror::Error;
use url::Url;

pub use secrecy;

#[derive(Clone)]
/// Shared ingress HTTP client used by generated service/object/workflow clients.
///
/// A `Client` holds a validated [`ServerUrl`] and optional [`AuthToken`].
/// It is cheap to clone and can be reused across threads.
///
/// # Example
///
/// ```no_run
/// # use restate_sdk::ingress::{AuthToken, Client, ServerUrl};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let server_url: ServerUrl = "https://api.example.com".try_into()?;
/// let token = AuthToken::new("token".to_string().into())?;
/// let client = Client::new(server_url, Some(token));
/// # let _ = client;
/// # Ok(())
/// # }
/// ```
pub struct Client {
    server_url: ServerUrl,
    auth_token: Option<AuthToken>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client(..)")
    }
}

impl Client {
    pub fn new(server_url: ServerUrl, auth_token: Option<AuthToken>) -> Client {
        Client {
            server_url,
            auth_token,
        }
    }

    pub fn service_client<C>(&self) -> C::Request<'_>
    where
        C: builder::IntoServiceRequest,
    {
        C::create_request(self)
    }

    pub fn object_client<C>(&self, key: impl Into<String>) -> C::Request<'_>
    where
        C: builder::IntoObjectRequest,
    {
        C::create_request(self, key.into())
    }

    pub fn workflow_client<C>(&self, key: impl Into<String>) -> C::Request<'_>
    where
        C: builder::IntoWorkflowRequest,
    {
        C::create_request(self, key.into())
    }
}

#[derive(Debug, Clone)]
/// Validated base URL for ingress requests.
///
/// `ServerUrl` must be an absolute HTTP(S) URL with a host and without query or
/// fragment components. It is used as the base for service, object, and workflow
/// request paths.
///
/// # Example
///
/// ```
/// # use restate_sdk::ingress::ServerUrl;
/// # use http::Uri;
/// # use http::uri::Authority;
/// # use url::Url;
/// # use std::net::SocketAddr;
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let from_str: ServerUrl = "http://localhost:8080".try_into()?;
///     let from_string: ServerUrl = "http://localhost:8080".to_string().try_into()?;
///     let from_url: ServerUrl = Url::parse("http://localhost:8080")?.try_into()?;
///     let uri: Uri = "http://localhost:8080".parse()?;
///     let from_uri: ServerUrl = uri.try_into()?;
///
///     // (defaults scheme to http)
///     let authority: Authority = "localhost:8080".parse()?;
///     let from_authority: ServerUrl = authority.try_into()?;
///     let socket: SocketAddr = "127.0.0.1:8080".parse()?;
///     let from_socket: ServerUrl = socket.try_into()?;
/// #   let _ = (from_str, from_string, from_url, from_uri, from_authority, from_socket);
/// #   Ok(())
/// # }
/// ```
pub struct ServerUrl(Url);

impl ServerUrl {
    pub fn build_for_path(&self, request_path: &str) -> Result<Url, RequestError> {
        Ok(self.0.join(request_path)?)
    }

    pub fn build_for_keyed(&self, service: &str, key: &str, handler: &str) -> Url {
        let mut request_url = self.0.clone();
        {
            let mut path = request_url.path_segments_mut().unwrap();
            path.pop_if_empty();
            path.push(service);
            path.push(key);
            path.push(handler);
        }
        request_url
    }
}

impl TryFrom<Url> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        if value.cannot_be_a_base()
            || value.host_str().is_none()
            || (value.scheme() != "http" && value.scheme() != "https")
            || value.query().is_some()
            || value.fragment().is_some()
        {
            return Err(url::ParseError::RelativeUrlWithoutBase);
        }
        Ok(Self(value))
    }
}

impl TryFrom<std::net::SocketAddr> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: std::net::SocketAddr) -> Result<Self, Self::Error> {
        Self::try_from(Authority::try_from(value.to_string().as_str()).unwrap())
    }
}

impl TryFrom<Authority> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: Authority) -> Result<Self, Self::Error> {
        let base_uri = http::Uri::builder()
            .scheme("http")
            .authority(value.as_str())
            .path_and_query("/")
            .build()
            .unwrap();
        Self::try_from(base_uri)
    }
}

impl TryFrom<Uri> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: Uri) -> Result<Self, Self::Error> {
        Self::try_from(value.to_string())
    }
}

impl TryFrom<String> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<&str> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = Url::parse(value)?;
        Self::try_from(url)
    }
}

#[derive(Clone, Debug)]
/// Bearer token used for ingress authentication.
///
/// This wraps [`SecretString`] and builds an `Authorization: Bearer ...` header.
/// Header values are marked sensitive before insertion.
pub struct AuthToken(SecretString);

impl AuthToken {
    pub fn new(token: SecretString) -> Result<Self, InvalidHeaderValue> {
        let _ = Self::parse_header_value(&token)?;
        Ok(Self(token))
    }

    pub fn to_request_header(&self) -> (HeaderName, HeaderValue) {
        let mut authorization = Self::parse_header_value(&self.0).unwrap();
        authorization.set_sensitive(true);
        (AUTHORIZATION, authorization)
    }

    fn parse_header_value(token: &SecretString) -> Result<HeaderValue, InvalidHeaderValue> {
        HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
    }
}

impl TryFrom<SecretString> for AuthToken {
    type Error = InvalidHeaderValue;

    fn try_from(value: SecretString) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Error)]
/// Error returned while building, sending, or decoding an ingress request.
pub enum RequestError {
    /// JSON serialization or deserialization failed for request/response payloads.
    #[error("request JSON serde failed: {0}")]
    Serde(#[from] serde_json::Error),
    /// Request URL construction failed (invalid base URL or request path composition).
    #[error("request invalid path: {0}")]
    InvalidPath(#[from] url::ParseError),
    /// Transport-layer request execution failed in the configured executor.
    #[error("request failed: {0}")]
    Request(String),
    /// Invalid header key or value was provided when building request metadata.
    #[error("invalid header: {0}")]
    InvalidHeader(String),
    /// The server returned a non-success status code; body contains response text.
    #[error("non-success status {status}: {body}")]
    Status { status: u16, body: String },
}

impl From<std::convert::Infallible> for RequestError {
    fn from(value: std::convert::Infallible) -> Self {
        match value {}
    }
}

pub struct Request<Res = ()> {
    request: Result<executor::HttpRequest, RequestError>,
    _res: PhantomData<Res>,
}

/// Fluent request builder returned by generated ingress clients.
///
/// `Request` lets callers add headers, idempotency, timeout, and delay before
/// executing the HTTP call via [`Request::call`].
impl<Res> From<RequestError> for Request<Res> {
    fn from(value: RequestError) -> Self {
        Self {
            request: Err(value),
            _res: PhantomData,
        }
    }
}

impl<Res> Request<Res> {
    fn from_request_result(request: Result<executor::HttpRequest, RequestError>) -> Self {
        Self {
            request,
            _res: PhantomData,
        }
    }

    /// Adds a single HTTP header to the ingress request.
    /// See the module-level typed client example for end-to-end usage.
    pub fn header<K, V>(mut self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.request = match self.request {
            Ok(mut request) => {
                let name = match HeaderName::try_from(key) {
                    Ok(name) => name,
                    Err(err) => return RequestError::InvalidHeader(err.into().to_string()).into(),
                };
                let value = match HeaderValue::try_from(value) {
                    Ok(value) => value,
                    Err(err) => return RequestError::InvalidHeader(err.into().to_string()).into(),
                };
                request.headers.insert(name, value);
                Ok(request)
            }
            Err(err) => Err(err),
        };
        self
    }

    /// Adds a map of HTTP headers to the ingress request.
    /// See the module-level typed client example for end-to-end usage.
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        if let Ok(mut request) = self.request {
            request.headers.extend(headers);
            self.request = Ok(request);
        }
        self
    }

    /// Sets a per-request timeout for the ingress call.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        if let Ok(mut request) = self.request {
            request.timeout = Some(timeout);
            self.request = Ok(request);
        }
        self
    }

    /// Sets Restate's idempotency key for this ingress request.
    pub fn idempotency_key(mut self, idempotency_key: impl AsRef<str>) -> Self {
        if let Ok(mut request) = self.request {
            let value = match HeaderValue::from_str(idempotency_key.as_ref()) {
                Ok(value) => value,
                Err(err) => return RequestError::InvalidHeader(err.to_string()).into(),
            };
            let name = HeaderName::from_static("idempotency-key");
            request.headers.insert(name, value);
            self.request = Ok(request);
        }
        self
    }

    /// Adds a Restate ingress `delay` query parameter to defer invocation.
    pub fn delay(mut self, delay: Duration) -> Self {
        if let Ok(mut request) = self.request {
            request
                .url
                .query_pairs_mut()
                .append_pair("delay", &format!("{}ms", delay.as_millis()));
            self.request = Ok(request);
        }
        self
    }

    /// Sends the request and deserializes the JSON response into `Res`.
    ///
    /// Non-success HTTP responses are returned as [`RequestError::Status`].
    pub async fn call(self, executor: &dyn executor::Executor) -> Result<Res, RequestError>
    where
        Res: DeserializeOwned,
    {
        let request = self.request?;

        let response = executor.execute(request).await?;
        if response.status < 200 || response.status > 299 {
            let status = response.status;
            let body = String::from_utf8_lossy(response.body.as_ref()).into_owned();
            return Err(RequestError::Status { status, body });
        }
        let body = response.body;
        let body = if body.is_empty() {
            b"null" as &[u8]
        } else {
            body.as_ref()
        };
        let response = serde_json::from_slice::<Res>(body)?;
        Ok(response)
    }
}

pub mod executor {
    use super::RequestError;
    use http::HeaderMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;
    use url::Url;

    /// Transport abstraction for executing ingress HTTP requests.
    ///
    /// Implement this trait to plug in a custom HTTP client backend.
    /// The ingress client builds a [`HttpRequest`] and delegates delivery to this trait.
    ///
    /// For reqwest, enable the `reqwest-client` feature and pass a `reqwest::Client`
    /// directly to [`crate::ingress::Request::call`].
    #[derive(Clone, Debug)]
    /// Transport-agnostic ingress request payload produced by the ingress builder.
    pub struct HttpRequest {
        pub url: Url,
        pub headers: HeaderMap,
        pub body: bytes::Bytes,
        pub timeout: Option<Duration>,
    }

    #[derive(Debug)]
    /// Transport-agnostic ingress response returned by an [`Executor`].
    pub struct HttpResponse {
        pub status: u16,
        pub body: bytes::Bytes,
    }

    /// Executes an ingress [`HttpRequest`] and returns an [`HttpResponse`].
    ///
    /// This trait is object-safe and intended for dynamic dispatch in
    /// [`crate::ingress::Request::call`].
    pub trait Executor: Send + Sync {
        fn execute<'a>(
            &'a self,
            request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, RequestError>> + Send + 'a>>;
    }
}

#[doc(hidden)]
pub mod builder {
    use super::executor::HttpRequest;
    use super::{Client, ServerUrl};
    use crate::ingress::{Request, RequestError};
    use crate::serde::Serialize;
    use http::header::CONTENT_TYPE;
    use http::header::HeaderMap;
    use http::header::HeaderValue;
    use url::Url;

    pub trait IntoServiceRequest: Sized {
        type Request<'a>;

        fn create_request<'a>(client: &'a Client) -> Self::Request<'a>;
    }

    pub trait IntoObjectRequest: Sized {
        type Request<'a>;

        fn create_request<'a>(client: &'a Client, key: String) -> Self::Request<'a>;
    }

    pub trait IntoWorkflowRequest: Sized {
        type Request<'a>;

        fn create_request<'a>(client: &'a Client, key: String) -> Self::Request<'a>;
    }

    pub fn service<Req, Res>(client: &Client, request_path: &str, req: Req) -> Request<Res>
    where
        Req: Serialize,
        RequestError: From<Req::Error>,
    {
        Request::from_request_result(build_post(
            req,
            |server_url| server_url.build_for_path(request_path),
            &client.server_url,
            client,
        ))
    }

    pub fn object<Req, Res>(
        client: &Client,
        service: &str,
        key: &str,
        handler: &str,
        req: Req,
    ) -> Request<Res>
    where
        Req: Serialize,
        RequestError: From<Req::Error>,
    {
        keyed(client, service, key, handler, req)
    }

    pub fn workflow<Req, Res>(
        client: &Client,
        service: &str,
        key: &str,
        handler: &str,
        req: Req,
    ) -> Request<Res>
    where
        Req: Serialize,
        RequestError: From<Req::Error>,
    {
        keyed(client, service, key, handler, req)
    }

    fn keyed<Req, Res>(
        client: &Client,
        service: &str,
        key: &str,
        handler: &str,
        req: Req,
    ) -> Request<Res>
    where
        Req: Serialize,
        RequestError: From<Req::Error>,
    {
        Request::from_request_result(build_post(
            req,
            |server_url| Ok(server_url.build_for_keyed(service, key, handler)),
            &client.server_url,
            client,
        ))
    }

    fn build_post<Req>(
        req: Req,
        make_url: impl FnOnce(&ServerUrl) -> Result<Url, RequestError>,
        server_url: &ServerUrl,
        client: &Client,
    ) -> Result<HttpRequest, RequestError>
    where
        Req: Serialize,
        RequestError: From<Req::Error>,
    {
        let request_url = make_url(server_url)?;
        let body = req.serialize()?;
        let mut headers = HeaderMap::new();
        if !body.is_empty() {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        if let Some(auth_token) = &client.auth_token {
            let (name, value) = auth_token.to_request_header();
            headers.insert(name, value);
        }

        Ok(HttpRequest {
            url: request_url,
            headers,
            body,
            timeout: None,
        })
    }
}

#[cfg(feature = "reqwest-client")]
pub mod reqwest {
    use super::RequestError;
    use super::executor::{Executor as IngressExecutor, HttpRequest, HttpResponse};
    use std::future::Future;
    use std::pin::Pin;

    pub use ::reqwest;

    impl IngressExecutor for ::reqwest::Client {
        fn execute<'a>(
            &'a self,
            request: HttpRequest,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, RequestError>> + Send + 'a>> {
            Box::pin(execute_with_client(self, request))
        }
    }

    async fn execute_with_client(
        client: &::reqwest::Client,
        request: HttpRequest,
    ) -> Result<HttpResponse, RequestError> {
        let mut req = client
            .post(request.url)
            .headers(request.headers)
            .body(request.body);
        if let Some(timeout) = request.timeout {
            req = req.timeout(timeout);
        }

        let response = req.send().await.map_err(request_error)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(request_error)?;
        Ok(HttpResponse { status, body })
    }

    fn request_error(err: ::reqwest::Error) -> RequestError {
        RequestError::Request(err.to_string())
    }
}
