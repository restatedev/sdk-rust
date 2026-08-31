//! # Ingress client
//!
//! Ingress clients invoke Restate handlers from outside a Restate service. The impl-block service,
//! virtual-object, and workflow macros generate a typed `<Type>IngressClient` for their handlers.
//! The ingress client requires Restate 1.7 or newer.
//!
//! ## Using reqwest
//!
//! Enable the `reqwest-client` feature to use [`ReqwestClient`]:
//!
//! ```rust,no_run
//! # use restate_sdk::prelude::*;
//! # struct Greeter;
//! # #[restate_sdk::service]
//! # impl Greeter {
//! #     #[handler]
//! #     async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
//! #         Ok(format!("Hello, {name}!"))
//! #     }
//! # }
//! # #[cfg(feature = "reqwest-client")]
//! # async fn call_greeter() -> Result<(), Box<dyn std::error::Error>> {
//! use restate_sdk::ingress::ReqwestClient;
//!
//! let client = ReqwestClient::connect("http://localhost:8080".parse()?)?;
//! let greeter = GreeterIngressClient::from_client(client);
//! let greeting = greeter
//!     .greet("Ada".to_owned())
//!     .call()
//!     .await?
//!     .into_body()?;
//!
//! println!("{greeting}");
//! # Ok(())
//! # }
//! ```
//!
//! ## Using another HTTP client
//!
//! Implement [`RequestExecutor`] for your HTTP client and pass it to [`Client::new`].

use std::error::Error;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

pub use crate::context::RequestTarget;
use crate::serde::{Deserialize, PayloadMetadata, Serialize};
use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE, InvalidHeaderValue};
use http::uri::PathAndQuery;
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, Request as HttpRequest, Response as HttpResponse,
    Response, StatusCode, Uri,
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");
const X_RESTATE_ID: HeaderName = HeaderName::from_static("x-restate-id");
const X_RESTATE_LIMIT_KEY: HeaderName = HeaderName::from_static("x-restate-limit-key");
const OUTPUT_NOT_READY: u16 = 470;

const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Executes one fully buffered HTTP request/response exchange.
///
/// Implementations must return the complete response body. Streaming and transport-native body
/// types intentionally do not cross this boundary.
pub trait RequestExecutor: Send + Sync + 'static {
    type Error: Error + Send + Sync + 'static;

    fn execute(
        &self,
        request: HttpRequest<Bytes>,
    ) -> impl Future<Output = Result<HttpResponse<Bytes>, Self::Error>> + Send;
}

/// Error returned when constructing an ingress client.
#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("base URI must include a scheme and authority")]
    RelativeBaseUri,
    #[error("base URI must not include a query")]
    BaseUriHasQuery,
    #[error("Restate auth token is not valid as an HTTP header value")]
    InvalidAuthToken {
        #[source]
        source: InvalidHeaderValue,
    },
}

/// Error produced while constructing, executing, or decoding an ingress request.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("ingress transport failed: {source}")]
    Transport {
        #[source]
        source: BoxError,
    },
    #[error("failed to construct ingress request: {source}")]
    Request {
        #[source]
        source: BoxError,
    },
    #[error("failed to serialize ingress request: {source}")]
    Serialization {
        #[source]
        source: BoxError,
    },
    #[error("invalid ingress request: {message}")]
    InvalidRequest { message: String },
    #[error("malformed ingress response: {message}")]
    Protocol {
        message: String,
        response: Box<HttpResponse<Bytes>>,
    },
    #[error("ingress returned HTTP status {status}")]
    Status {
        status: StatusCode,
        response: Box<HttpResponse<Bytes>>,
    },
    #[error("failed to decode ingress response payload: {source}")]
    PayloadDecode {
        #[source]
        source: BoxError,
        response: Box<HttpResponse<Bytes>>,
    },
}

impl ClientError {
    /// Returns the original buffered HTTP response when the failure happened after transport.
    pub fn response(&self) -> Option<&HttpResponse<Bytes>> {
        match self {
            Self::Protocol { response, .. }
            | Self::Status { response, .. }
            | Self::PayloadDecode { response, .. } => Some(response),
            Self::Transport { .. }
            | Self::Request { .. }
            | Self::Serialization { .. }
            | Self::InvalidRequest { .. } => None,
        }
    }

    /// Consumes the error and returns its original buffered HTTP response, if any.
    pub fn into_response(self) -> Option<HttpResponse<Bytes>> {
        match self {
            Self::Protocol { response, .. }
            | Self::Status { response, .. }
            | Self::PayloadDecode { response, .. } => Some(*response),
            Self::Transport { .. }
            | Self::Request { .. }
            | Self::Serialization { .. }
            | Self::InvalidRequest { .. } => None,
        }
    }
}

struct ClientState<E> {
    scheme: http::uri::Scheme,
    authority: http::uri::Authority,
    base_path: String,
    executor: E,
}

/// A transport-neutral Restate ingress client.
pub struct Client<E> {
    state: Arc<ClientState<E>>,
    default_headers: Arc<HeaderMap>,
}

impl<E> Clone for Client<E> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            default_headers: Arc::clone(&self.default_headers),
        }
    }
}

impl<E: RequestExecutor> Client<E> {
    /// Creates a client with no default headers.
    pub fn new(base_uri: Uri, executor: E) -> Result<Self, ClientBuildError> {
        Self::builder(base_uri, executor).build()
    }

    /// Creates a client builder.
    pub fn builder(base_uri: Uri, executor: E) -> ClientBuilder<E> {
        ClientBuilder {
            base_uri,
            executor,
            default_headers: HeaderMap::new(),
        }
    }

    /// Adds a raw Restate Cloud authentication token to every request.
    ///
    /// This method prepends `Bearer ` and installs the resulting `Authorization` header as a
    /// default. An `Authorization` header set on an individual request overrides it.
    ///
    /// # Errors
    ///
    /// Returns [`ClientBuildError::InvalidAuthToken`] when the token cannot be represented in an
    /// HTTP header.
    pub fn with_restate_auth_token(
        mut self,
        token: impl AsRef<str>,
    ) -> Result<Self, ClientBuildError> {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", token.as_ref()))
            .map_err(|source| ClientBuildError::InvalidAuthToken { source })?;
        value.set_sensitive(true);
        Arc::make_mut(&mut self.default_headers).insert(AUTHORIZATION, value);
        Ok(self)
    }

    /// Creates a typed request for a service handler.
    pub fn request<Req, Res>(&self, target: RequestTarget, input: Req) -> Request<E, Req, Res> {
        Request {
            client: self.clone(),
            target,
            input,
            headers: HeaderMap::new(),
            idempotency_key: None,
            scope: None,
            limit_key: None,
            response: PhantomData,
        }
    }

    /// Creates a typed handle for a persisted or externally obtained invocation ID.
    pub fn invocation_handle<Res>(
        &self,
        invocation_id: impl Into<InvocationId>,
    ) -> InvocationHandle<E, Res> {
        InvocationHandle {
            client: self.clone(),
            invocation_id: Arc::new(invocation_id.into()),
            response: PhantomData,
        }
    }

    fn uri(&self, path_and_query: String) -> Result<Uri, ClientError> {
        let path_and_query = PathAndQuery::try_from(path_and_query).map_err(request_error)?;
        Uri::builder()
            .scheme(self.state.scheme.clone())
            .authority(self.state.authority.clone())
            .path_and_query(path_and_query)
            .build()
            .map_err(request_error)
    }

    fn path(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.state.base_path)
    }

    async fn execute(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, ClientError> {
        self.state
            .executor
            .execute(request)
            .await
            .map_err(|source| ClientError::Transport {
                source: Box::new(source),
            })
    }

    async fn get(&self, path: String) -> Result<HttpResponse<Bytes>, ClientError> {
        let mut request = HttpRequest::builder()
            .method(Method::GET)
            .uri(self.uri(self.path(&path))?)
            .body(Bytes::new())
            .map_err(request_error)?;
        *request.headers_mut() = self.default_headers.as_ref().clone();
        self.execute(request).await
    }
}

/// Builder for [`Client`].
pub struct ClientBuilder<E> {
    base_uri: Uri,
    executor: E,
    default_headers: HeaderMap,
}

impl<E: RequestExecutor> ClientBuilder<E> {
    /// Adds or replaces a header included in every request.
    pub fn default_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.default_headers.insert(name, value);
        self
    }

    /// Validates the base URI and builds the client.
    pub fn build(self) -> Result<Client<E>, ClientBuildError> {
        let Some(scheme) = self.base_uri.scheme().cloned() else {
            return Err(ClientBuildError::RelativeBaseUri);
        };
        let Some(authority) = self.base_uri.authority().cloned() else {
            return Err(ClientBuildError::RelativeBaseUri);
        };
        if self.base_uri.query().is_some() {
            return Err(ClientBuildError::BaseUriHasQuery);
        }

        let path = self.base_uri.path();
        let base_path = if path == "/" {
            String::new()
        } else {
            path.trim_end_matches('/').to_owned()
        };

        Ok(Client {
            state: Arc::new(ClientState {
                scheme,
                authority,
                base_path,
                executor: self.executor,
            }),
            default_headers: Arc::new(self.default_headers),
        })
    }
}

fn encoded_target_path(target: &RequestTarget) -> String {
    match target {
        RequestTarget::Service { name, handler } => {
            format!("{}/{}", encode_segment(name), encode_segment(handler))
        }
        RequestTarget::Object { name, key, handler }
        | RequestTarget::Workflow { name, key, handler } => format!(
            "{}/{}/{}",
            encode_segment(name),
            encode_segment(key),
            encode_segment(handler)
        ),
    }
}

/// An owned typed ingress request builder.
pub struct Request<E, Req, Res = ()> {
    client: Client<E>,
    target: RequestTarget,
    input: Req,
    headers: HeaderMap,
    idempotency_key: Option<String>,
    scope: Option<String>,
    limit_key: Option<String>,
    response: PhantomData<fn() -> Res>,
}

impl<E, Req, Res> Request<E, Req, Res> {
    /// Adds or replaces a request header.
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Overrides the outgoing content type.
    pub fn content_type(mut self, value: HeaderValue) -> Self {
        self.headers.insert(CONTENT_TYPE, value);
        self
    }

    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    pub fn limit_key(mut self, key: impl Into<String>) -> Self {
        self.limit_key = Some(key.into());
        self
    }

    async fn call_with_metadata(
        self,
        metadata: Option<RequestMetadata>,
    ) -> Result<CallResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
        Req: Serialize,
    {
        let (client, request) = self.into_http_request(InvokeType::Call, None, metadata)?;
        let response = client.execute(request).await?;
        let invocation_id = match invocation_id_header(&response) {
            Ok(invocation_id) => invocation_id,
            Err(message) => {
                return Err(ClientError::Protocol {
                    message,
                    response: Box::new(response),
                });
            }
        };
        let handle = client.invocation_handle(invocation_id);
        Ok(CallResponse { response, handle })
    }

    async fn send_with_delay_and_metadata(
        self,
        delay: Option<Duration>,
        metadata: Option<RequestMetadata>,
    ) -> Result<SendResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
        Req: Serialize,
    {
        let (client, request) = self.into_http_request(InvokeType::Send, delay, metadata)?;
        let response = client.execute(request).await?;
        if response.status() != StatusCode::ACCEPTED {
            return Err(status_error(response));
        }

        let invocation_id = match invocation_id_header(&response) {
            Ok(invocation_id) => invocation_id,
            Err(message) => {
                return Err(ClientError::Protocol {
                    message,
                    response: Box::new(response),
                });
            }
        };
        let acknowledgement: SendAcknowledgement = match serde_json::from_slice(response.body()) {
            Ok(acknowledgement) => acknowledgement,
            Err(source) => {
                return Err(ClientError::Protocol {
                    message: format!("invalid send acknowledgement: {source}"),
                    response: Box::new(response),
                });
            }
        };
        if acknowledgement.invocation_id.is_empty() {
            return Err(ClientError::Protocol {
                message: "send acknowledgement contains an empty invocationId".to_owned(),
                response: Box::new(response),
            });
        }

        Ok(SendResponse {
            handle: client.invocation_handle(invocation_id),
            status: acknowledgement.status.into(),
        })
    }

    fn into_http_request(
        self,
        invoke_type: InvokeType,
        delay: Option<Duration>,
        metadata: Option<RequestMetadata>,
    ) -> Result<(Client<E>, HttpRequest<Bytes>), ClientError>
    where
        E: RequestExecutor,
        Req: Serialize,
    {
        let Self {
            client,
            target,
            input,
            headers,
            idempotency_key,
            scope,
            limit_key,
            response: _,
        } = self;

        if limit_key.as_deref().is_some_and(|key| !key.is_empty()) && scope.is_none() {
            return Err(ClientError::InvalidRequest {
                message: "a limit key requires a scope".to_owned(),
            });
        }
        if let Some(limit_key) = limit_key.as_deref() {
            validate_limit_key(limit_key)?;
        }

        let body = input
            .serialize()
            .map_err(|source| ClientError::Serialization {
                source: Box::new(source),
            })?;
        let mut merged_headers = client.default_headers.as_ref().clone();
        for (name, value) in headers {
            if let Some(name) = name {
                merged_headers.insert(name, value);
            }
        }
        if let Some(key) = idempotency_key {
            let value = HeaderValue::try_from(key).map_err(request_error)?;
            merged_headers.insert(IDEMPOTENCY_KEY, value);
        }
        if let Some(key) = limit_key {
            let value = HeaderValue::try_from(key).map_err(request_error)?;
            merged_headers.insert(X_RESTATE_LIMIT_KEY, value);
        }

        if let Some(metadata) = metadata {
            let should_set_content_type = !body.is_empty()
                || metadata.set_content_type_if_empty
                || metadata.input_is_required;
            if should_set_content_type && !merged_headers.contains_key(CONTENT_TYPE) {
                let value = HeaderValue::try_from(metadata.content_type).map_err(request_error)?;
                merged_headers.insert(CONTENT_TYPE, value);
            }
        }

        let operation = match invoke_type {
            InvokeType::Call => "call",
            InvokeType::Send => "send",
        };
        let prefix = match scope {
            Some(scope) => format!("/restate/scope/{}/{operation}", encode_segment(&scope)),
            None => format!("/restate/{operation}"),
        };
        let mut path_and_query = client.path(&format!("{prefix}/{}", encoded_target_path(&target)));
        if let Some(delay) = delay {
            path_and_query.push_str("?delay=");
            path_and_query.push_str(&format_iso8601_duration(delay));
        }

        let mut request = HttpRequest::builder()
            .method(Method::POST)
            .uri(client.uri(path_and_query)?)
            .body(body)
            .map_err(request_error)?;
        *request.headers_mut() = merged_headers;
        Ok((client, request))
    }
}

struct RequestMetadata {
    content_type: &'static str,
    set_content_type_if_empty: bool,
    input_is_required: bool,
}

impl RequestMetadata {
    fn for_payload<T: PayloadMetadata>() -> Self {
        let output = T::output_metadata();
        Self {
            content_type: output.content_type,
            set_content_type_if_empty: output.set_content_type_if_empty,
            input_is_required: T::input_metadata().is_required,
        }
    }
}

impl<E, Req, Res> Request<E, Req, Res>
where
    Req: Serialize + PayloadMetadata,
{
    /// Invokes a handler and waits for its completed response.
    pub async fn call(self) -> Result<CallResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.call_with_metadata(Some(RequestMetadata::for_payload::<Req>()))
            .await
    }

    /// Submits a one-way invocation.
    pub async fn send(self) -> Result<SendResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.send_with_delay_and_metadata(None, Some(RequestMetadata::for_payload::<Req>()))
            .await
    }

    /// Submits a one-way invocation to execute after `delay`.
    pub async fn send_after(self, delay: Duration) -> Result<SendResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.send_with_delay_and_metadata(Some(delay), Some(RequestMetadata::for_payload::<Req>()))
            .await
    }
}

// This impl is deliberately disjoint from the metadata-bearing impl above: `()` does not
// implement `PayloadMetadata`, and downstream crates cannot add that impl under Rust's orphan
// rules. Unit remains the generated client's no-input marker without changing global metadata or
// discovery behavior.
impl<E, Res> Request<E, (), Res> {
    /// Invokes a no-input handler and waits for its completed response.
    pub async fn call(self) -> Result<CallResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.call_with_metadata(None).await
    }

    /// Submits a one-way no-input invocation.
    pub async fn send(self) -> Result<SendResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.send_with_delay_and_metadata(None, None).await
    }

    /// Submits a one-way no-input invocation to execute after `delay`.
    pub async fn send_after(self, delay: Duration) -> Result<SendResponse<E, Res>, ClientError>
    where
        E: RequestExecutor,
    {
        self.send_with_delay_and_metadata(Some(delay), None).await
    }
}

#[derive(Clone, Copy)]
enum InvokeType {
    Call,
    Send,
}

/// A Restate invocation identifier.
pub type InvocationId = String;

/// A completed call response and its invocation handle.
#[must_use = "call responses must be consumed with into_body or used for their invocation handle"]
pub struct CallResponse<E, Res> {
    response: HttpResponse<Bytes>,
    handle: InvocationHandle<E, Res>,
}

impl<E, Res> CallResponse<E, Res> {
    /// Returns an invocation handle for this request.
    pub fn invocation_handle(&self) -> InvocationHandle<E, Res> {
        self.handle.clone()
    }

    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &HeaderMap<HeaderValue> {
        self.response.headers()
    }

    /// Get the http response
    pub fn into_http_response(self) -> Result<HttpResponse<Res>, ClientError>
    where
        Res: Deserialize,
    {
        decode_response(self.response)
    }

    /// Get the http response with the raw payload
    pub fn into_raw_http_response(self) -> HttpResponse<Bytes> {
        self.response
    }

    /// Checks if response is 200 OK, then returns the response body.
    pub fn into_body(self) -> Result<Res, ClientError>
    where
        Res: Deserialize,
    {
        if self.response.status() != StatusCode::OK {
            return Err(status_error(self.response));
        }
        decode_body(self.response)
    }
}

/// The result of submitting a one-way invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendStatus {
    Accepted,
    PreviouslyAccepted,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendAcknowledgement {
    invocation_id: String,
    status: SendStatusWire,
}

#[derive(serde::Deserialize)]
enum SendStatusWire {
    Accepted,
    PreviouslyAccepted,
}

impl From<SendStatusWire> for SendStatus {
    fn from(value: SendStatusWire) -> Self {
        match value {
            SendStatusWire::Accepted => Self::Accepted,
            SendStatusWire::PreviouslyAccepted => Self::PreviouslyAccepted,
        }
    }
}

/// A successful one-way invocation acknowledgement.
pub struct SendResponse<E, Res> {
    handle: InvocationHandle<E, Res>,
    status: SendStatus,
}

impl<E, Res> SendResponse<E, Res> {
    /// Returns the invocation handle to interact with this invocation.
    pub fn invocation_handle(&self) -> InvocationHandle<E, Res> {
        self.handle.clone()
    }

    pub fn send_status(&self) -> SendStatus {
        self.status
    }
}

/// A typed handle to a Restate invocation.
pub struct InvocationHandle<E, Res> {
    client: Client<E>,
    invocation_id: Arc<InvocationId>,
    response: PhantomData<fn() -> Res>,
}

impl<E, Res> Clone for InvocationHandle<E, Res> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            invocation_id: self.invocation_id.clone(),
            response: PhantomData,
        }
    }
}

impl<E, Res> InvocationHandle<E, Res>
where
    E: RequestExecutor,
    Res: Deserialize,
{
    pub fn invocation_id(&self) -> &InvocationId {
        self.invocation_id.as_ref()
    }

    /// Waits for the invocation to finish and decodes its output.
    pub async fn attach(&self) -> Result<HttpResponse<Res>, ClientError> {
        let response = self
            .client
            .get(format!(
                "/restate/attach/{}",
                encode_segment(self.invocation_id.as_str())
            ))
            .await?;
        if response.status() != StatusCode::OK {
            return Err(status_error(response));
        }
        decode_response(response)
    }

    /// Peeks at the invocation output without waiting for completion.
    pub async fn output(&self) -> Result<HttpResponse<Output<Res>>, ClientError> {
        let response = self
            .client
            .get(format!(
                "/restate/output/{}",
                encode_segment(self.invocation_id.as_str())
            ))
            .await?;
        match response.status() {
            StatusCode::OK => decode_response(response).map(|response| response.map(Output::Ready)),
            status if status.as_u16() == OUTPUT_NOT_READY => Ok(response.map(|_| Output::NotReady)),
            _ => Err(status_error(response)),
        }
    }
}

/// Output state returned by [`InvocationHandle::output`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output<T> {
    Ready(T),
    NotReady,
}

fn encode_segment(segment: &str) -> String {
    utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string()
}

fn format_iso8601_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        return format!("PT{}S", duration.as_secs());
    }
    let fraction = format!("{:09}", duration.subsec_nanos());
    format!(
        "PT{}.{fraction}S",
        duration.as_secs(),
        fraction = fraction.trim_end_matches('0')
    )
}

fn validate_limit_key(key: &str) -> Result<(), ClientError> {
    if key.is_empty() {
        return Ok(());
    }
    let trimmed = key.strip_suffix('/').unwrap_or(key);
    let components: Vec<_> = trimmed.split('/').collect();
    if components.len() > 2 {
        return invalid_request("a limit key can contain at most two components");
    }
    for component in components {
        if component.is_empty() {
            return invalid_request("limit key components cannot be empty");
        }
        if component.len() > 36 {
            return invalid_request("limit key components cannot exceed 36 bytes");
        }
        if !component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        {
            return invalid_request(
                "limit key components may contain only ASCII letters, digits, '_', '.', and '-'",
            );
        }
    }
    Ok(())
}

fn invalid_request<T>(message: impl Into<String>) -> Result<T, ClientError> {
    Err(ClientError::InvalidRequest {
        message: message.into(),
    })
}

fn invocation_id_header(response: &HttpResponse<Bytes>) -> Result<InvocationId, String> {
    let value = response
        .headers()
        .get(&X_RESTATE_ID)
        .ok_or_else(|| "response is missing x-restate-id".to_owned())?;
    let value = value
        .to_str()
        .map_err(|_| "response has a non-ASCII x-restate-id".to_owned())?;
    if value.is_empty() {
        return Err("response has an empty x-restate-id".to_owned());
    }
    Ok(InvocationId::from(value))
}

fn request_error(source: impl Error + Send + Sync + 'static) -> ClientError {
    ClientError::Request {
        source: Box::new(source),
    }
}

fn status_error(response: HttpResponse<Bytes>) -> ClientError {
    ClientError::Status {
        status: response.status(),
        response: Box::new(response),
    }
}

fn decode_body<T: Deserialize>(response: HttpResponse<Bytes>) -> Result<T, ClientError> {
    let mut body = response.body().clone();
    T::deserialize(&mut body).map_err(|source| ClientError::PayloadDecode {
        source: Box::new(source),
        response: Box::new(response),
    })
}

fn decode_response<T: Deserialize>(
    response: HttpResponse<Bytes>,
) -> Result<HttpResponse<T>, ClientError> {
    let (parts, mut body) = response.into_parts();
    match T::deserialize(&mut body) {
        Ok(body) => Ok(HttpResponse::from_parts(parts, body)),
        Err(source) => Err(ClientError::PayloadDecode {
            source: Box::new(source),
            response: Box::new(Response::from_parts(parts, body)),
        }),
    }
}

#[cfg(feature = "reqwest-client")]
impl RequestExecutor for reqwest::Client {
    type Error = reqwest::Error;

    async fn execute(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, Self::Error> {
        let request: reqwest::Request = request.try_into()?;
        let response = self.execute(request).await?;
        let status = response.status();
        let version = response.version();
        let headers = response.headers().clone();
        let body = response.bytes().await?;

        let mut response = HttpResponse::new(body);
        *response.status_mut() = status;
        *response.version_mut() = version;
        *response.headers_mut() = headers;
        Ok(response)
    }
}

/// Reqwest-backed ingress client, available with the `reqwest-client` feature.
#[cfg(feature = "reqwest-client")]
pub type ReqwestClient = Client<reqwest::Client>;

#[cfg(feature = "reqwest-client")]
impl Client<reqwest::Client> {
    /// Connects using reqwest's default client configuration.
    pub fn connect(base_uri: Uri) -> Result<Self, ClientBuildError> {
        Self::new(base_uri, reqwest::Client::new())
    }
}
