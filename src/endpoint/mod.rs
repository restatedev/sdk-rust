mod builder;
mod context;
mod futures;
mod handler_state;

pub use builder::{Builder, HandlerOptions, ServiceOptions};

use crate::endpoint::futures::handler_state_aware::HandlerStateAwareFuture;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::service::Service;
use ::futures::future::BoxFuture;
use ::futures::{FutureExt, Stream, StreamExt, TryStreamExt};
use bytes::Bytes;
pub use context::{ContextInternal, InputMetadata};
use http::header::CONTENT_TYPE;
use http::{HeaderName, HeaderValue};
use http_body::{Body, Frame, SizeHint};
use http_body_util::{BodyExt, Either, Full};
use pin_project_lite::pin_project;
use restate_sdk_shared_core::{
    CoreVM, Error as CoreError, Header, HeaderMap, IdentityVerifier, ResponseHead, VerifyError, VM,
};
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::poll_fn;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use tokio::sync::mpsc;
use tracing::{info_span, warn, Instrument};

#[allow(clippy::declare_interior_mutable_const)]
const X_RESTATE_SERVER: HeaderName = HeaderName::from_static("x-restate-server");
const X_RESTATE_SERVER_VALUE: HeaderValue =
    HeaderValue::from_static(concat!("restate-sdk-rust/", env!("CARGO_PKG_VERSION")));
const DISCOVERY_CONTENT_TYPE_V2: &str = "application/vnd.restate.endpointmanifest.v2+json";
const DISCOVERY_CONTENT_TYPE_V3: &str = "application/vnd.restate.endpointmanifest.v3+json";
const DISCOVERY_CONTENT_TYPE_V4: &str = "application/vnd.restate.endpointmanifest.v4+json";

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

// TODO can we have the backtrace here?
/// Endpoint error. This encapsulates any error that happens within the SDK while processing a request.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(#[from] ErrorInner);

impl Error {
    /// New error for unknown handler
    pub fn unknown_handler(service_name: &str, handler_name: &str) -> Self {
        Self(ErrorInner::UnknownServiceHandler(
            service_name.to_owned(),
            handler_name.to_owned(),
        ))
    }

    /// Returns the HTTP status code for this error.
    pub fn status_code(&self) -> u16 {
        match &self.0 {
            ErrorInner::VM(e) => e.code(),
            ErrorInner::UnknownService(_) | ErrorInner::UnknownServiceHandler(_, _) => 404,
            ErrorInner::Suspended
            | ErrorInner::UnexpectedOutputClosed
            | ErrorInner::UnexpectedValueVariantForSyscall { .. }
            | ErrorInner::Deserialization { .. }
            | ErrorInner::Serialization { .. }
            | ErrorInner::HandlerResult { .. } => 500,
            ErrorInner::FieldRequiresMinimumVersion { .. } => 500,
            ErrorInner::BadDiscoveryVersion(_) => 415,
            ErrorInner::Header { .. } | ErrorInner::BadPath { .. } => 400,
            ErrorInner::IdentityVerification(_) => 401,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ErrorInner {
    #[error("Received a request for unknown service '{0}'")]
    UnknownService(String),
    #[error("Received a request for unknown service handler '{0}/{1}'")]
    UnknownServiceHandler(String, String),
    #[error("Error when processing the request: {0:?}")]
    VM(#[from] CoreError),
    #[error("Error when verifying identity: {0:?}")]
    IdentityVerification(#[from] VerifyError),
    #[error("Cannot convert header '{0}', reason: {1}")]
    Header(String, #[source] BoxError),
    #[error("Cannot reply to discovery, got accept header '{0}' but currently supported discovery versions are v2 and v3")]
    BadDiscoveryVersion(String),
    #[error("The field '{0}' was set in the service/handler options, but it requires minimum discovery protocol version {1}")]
    FieldRequiresMinimumVersion(&'static str, u32),
    #[error("Bad path '{0}', expected either '/discover' or '/invoke/service/handler'")]
    BadPath(String),
    #[error("Suspended")]
    Suspended,
    #[error("Unexpected output closed")]
    UnexpectedOutputClosed,
    #[error("Unexpected value variant {variant} for syscall '{syscall}'")]
    UnexpectedValueVariantForSyscall {
        variant: &'static str,
        syscall: &'static str,
    },
    #[error("Failed to deserialize with '{syscall}': {err:?}'")]
    Deserialization {
        syscall: &'static str,
        #[source]
        err: BoxError,
    },
    #[error("Failed to serialize with '{syscall}': {err:?}'")]
    Serialization {
        syscall: &'static str,
        #[source]
        err: BoxError,
    },
    #[error("Handler failed with retryable error: {err:?}'")]
    HandlerResult {
        #[source]
        err: BoxError,
    },
}

impl From<CoreError> for Error {
    fn from(e: CoreError) -> Self {
        if e.is_suspended_error() {
            return ErrorInner::Suspended.into();
        }
        ErrorInner::from(e).into()
    }
}

struct BoxedService(
    Box<dyn Service<Future = BoxFuture<'static, Result<(), Error>>> + Send + Sync + 'static>,
);

impl BoxedService {
    pub fn new<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>> + Send + Sync + 'static,
    >(
        service: S,
    ) -> Self {
        Self(Box::new(service))
    }
}

impl Service for BoxedService {
    type Future = BoxFuture<'static, Result<(), Error>>;

    fn handle(&self, req: ContextInternal) -> Self::Future {
        self.0.handle(req)
    }
}

/// This struct encapsulates all the business logic to handle incoming requests to the SDK,
/// including service discovery, invocations and identity verification.
///
/// It internally wraps the provided services. This structure is cheaply cloneable.
#[derive(Clone)]
pub struct Endpoint(Arc<EndpointInner>);

impl Endpoint {
    /// Create a new builder for [`Endpoint`].
    pub fn builder() -> Builder {
        Builder::new()
    }
}

struct EndpointInner {
    svcs: HashMap<String, BoxedService>,
    discovery_services: Vec<crate::discovery::Service>,
    identity_verifier: IdentityVerifier,
}

#[derive(Default)]
pub(crate) enum ProtocolMode {
    #[allow(dead_code)]
    RequestResponse,
    #[default]
    BidiStream,
}

/// Options for [`Endpoint::handle`].
#[derive(Default)]
pub(crate) struct HandleOptions {
    pub(crate) protocol_mode: ProtocolMode,
}

impl Endpoint {
    /// Handle an [`http::Request`], producing an [`http::Response`].
    pub fn handle<B: Body<Data = Bytes, Error: Into<BoxError> + Send> + Send + 'static>(
        &self,
        req: http::Request<B>,
    ) -> http::Response<ResponseBody> {
        self.handle_with_options(req, HandleOptions::default())
    }

    /// Handle an [`http::Request`], producing an [`http::Response`].
    pub(crate) fn handle_with_options<
        B: Body<Data = Bytes, Error: Into<BoxError> + Send> + Send + 'static,
    >(
        &self,
        req: http::Request<B>,
        options: HandleOptions,
    ) -> http::Response<ResponseBody> {
        let (parts, body) = req.into_parts();
        let path = parts.uri.path();
        let headers = parts.headers;

        if let Err(e) = self.0.identity_verifier.verify_identity(&headers, path) {
            return error_response(ErrorInner::IdentityVerification(e));
        }

        let parts: Vec<&str> = path.split('/').collect();

        if parts.last() == Some(&"health") {
            return self.handle_health();
        }
        if parts.last() == Some(&"discover") {
            return self.handle_discovery(headers, options.protocol_mode);
        }

        // Parse service name/handler name
        let (svc_name, handler_name) = match parts.get(parts.len() - 3..) {
            None => return error_response(ErrorInner::BadPath(path.to_owned())),
            Some(last_elements) if last_elements[0] != "invoke" => {
                return error_response(ErrorInner::BadPath(path.to_owned()))
            }
            Some(last_elements) => (last_elements[1].to_owned(), last_elements[2].to_owned()),
        };

        // Prepare vm
        let vm = match CoreVM::new(headers, Default::default()) {
            Ok(vm) => vm,
            Err(e) => return error_response(e),
        };
        let ResponseHead {
            status_code,
            headers,
            ..
        } = vm.get_response_head();

        // Resolve service
        if !self.0.svcs.contains_key(&svc_name) {
            return error_response(ErrorInner::UnknownService(svc_name.to_owned()));
        }

        // Prepare handle_invocation future
        let input_receiver =
            InputReceiver::from_stream(body.into_data_stream().map_err(|e| e.into()));
        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let output_sender = OutputSender::from_channel(output_tx);
        let handle_invocation_fut = Box::pin(handle_invocation(
            svc_name,
            handler_name,
            vm,
            Arc::clone(&self.0),
            input_receiver,
            output_sender,
        ));

        // Wrap the invocation runner in the response
        // When the body is pulled, the invocation gets processed.
        let mut invocation_response_builder = http::Response::builder()
            .status(status_code)
            .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE);
        for Header { key, value } in headers {
            invocation_response_builder =
                invocation_response_builder.header(key.deref(), value.deref());
        }
        invocation_response_builder
            .body(
                Either::Right(InvocationRunnerBody {
                    fut: Some(handle_invocation_fut),
                    output_rx,
                    end_stream: false,
                })
                .into(),
            )
            .expect("Headers should be valid")
    }

    fn handle_health(&self) -> http::Response<ResponseBody> {
        simple_response(200, vec![], Bytes::default())
    }

    fn handle_discovery(
        &self,
        headers: http::HeaderMap,
        protocol_mode: ProtocolMode,
    ) -> http::Response<ResponseBody> {
        // Extract Accept header from request
        let accept_header = match headers
            .extract("accept")
            .map_err(|e| ErrorInner::Header("accept".to_owned(), Box::new(e)))
        {
            Ok(h) => h,
            Err(e) => return error_response(e),
        };

        // Negotiate discovery protocol version
        let mut version = 2;
        let mut content_type = DISCOVERY_CONTENT_TYPE_V2;
        if let Some(accept) = accept_header {
            if accept.contains(DISCOVERY_CONTENT_TYPE_V4) {
                version = 4;
                content_type = DISCOVERY_CONTENT_TYPE_V4;
            } else if accept.contains(DISCOVERY_CONTENT_TYPE_V3) {
                version = 3;
                content_type = DISCOVERY_CONTENT_TYPE_V3;
            } else if accept.contains(DISCOVERY_CONTENT_TYPE_V2) {
                version = 2;
                content_type = DISCOVERY_CONTENT_TYPE_V2;
            } else {
                return error_response(ErrorInner::BadDiscoveryVersion(accept.to_owned()));
            }
        }

        if let Err(e) = self.validate_discovery_request(version) {
            return error_response(e);
        }

        simple_response(
            200,
            vec![Header {
                key: "content-type".into(),
                value: content_type.into(),
            }],
            Bytes::from(
                serde_json::to_string(&crate::discovery::Endpoint {
                    lambda_compression: None,
                    max_protocol_version: 5,
                    min_protocol_version: 5,
                    protocol_mode: Some(match protocol_mode {
                        ProtocolMode::RequestResponse => {
                            crate::discovery::ProtocolMode::RequestResponse
                        }
                        ProtocolMode::BidiStream => crate::discovery::ProtocolMode::BidiStream,
                    }),
                    services: self.0.discovery_services.clone(),
                })
                .expect("Discovery should be serializable"),
            ),
        )
    }

    fn validate_discovery_request(&self, version: usize) -> Result<(), ErrorInner> {
        // Validate that new discovery fields aren't used with older protocol versions
        if version <= 3 {
            // Check for new discovery fields in version 3 that shouldn't be used in version 2 or lower
            for service in &self.0.discovery_services {
                if service.retry_policy_initial_interval.is_some()
                    || service.retry_policy_exponentiation_factor.is_some()
                    || service.retry_policy_max_interval.is_some()
                    || service.retry_policy_max_attempts.is_some()
                    || service.retry_policy_on_max_attempts.is_some()
                {
                    Err(ErrorInner::FieldRequiresMinimumVersion("retry_policy", 4))?;
                }

                for handler in &service.handlers {
                    if handler.retry_policy_initial_interval.is_some()
                        || handler.retry_policy_exponentiation_factor.is_some()
                        || handler.retry_policy_max_interval.is_some()
                        || handler.retry_policy_max_attempts.is_some()
                        || handler.retry_policy_on_max_attempts.is_some()
                    {
                        Err(ErrorInner::FieldRequiresMinimumVersion("retry_policy", 4))?;
                    }
                }
            }
        }
        if version <= 2 {
            // Check for new discovery fields in version 3 that shouldn't be used in version 2 or lower
            for service in &self.0.discovery_services {
                if service.inactivity_timeout.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion(
                        "inactivity_timeout",
                        3,
                    ))?;
                }
                if service.abort_timeout.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion("abort_timeout", 3))?;
                }
                if service.idempotency_retention.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion(
                        "idempotency_retention",
                        3,
                    ))?;
                }
                if service.journal_retention.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion(
                        "journal_retention",
                        3,
                    ))?;
                }
                if service.enable_lazy_state.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion(
                        "enable_lazy_state",
                        3,
                    ))?;
                }
                if service.ingress_private.is_some() {
                    Err(ErrorInner::FieldRequiresMinimumVersion(
                        "ingress_private",
                        3,
                    ))?;
                }

                for handler in &service.handlers {
                    if handler.inactivity_timeout.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "inactivity_timeout",
                            3,
                        ))?;
                    }
                    if handler.abort_timeout.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion("abort_timeout", 3))?;
                    }
                    if handler.idempotency_retention.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "idempotency_retention",
                            3,
                        ))?;
                    }
                    if handler.journal_retention.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "journal_retention",
                            3,
                        ))?;
                    }
                    if handler.workflow_completion_retention.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "workflow_retention",
                            3,
                        ))?;
                    }
                    if handler.enable_lazy_state.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "enable_lazy_state",
                            3,
                        ))?;
                    }
                    if handler.ingress_private.is_some() {
                        Err(ErrorInner::FieldRequiresMinimumVersion(
                            "ingress_private",
                            3,
                        ))?;
                    }
                }
            }
        }
        Ok(())
    }
}

type ResponseBodyInner = Either<Full<Bytes>, InvocationRunnerBody>;
pin_project! {
    pub struct ResponseBody {
        #[pin]
        inner: ResponseBodyInner
    }
}

impl From<ResponseBodyInner> for ResponseBody {
    fn from(e: ResponseBodyInner) -> Self {
        ResponseBody { inner: e }
    }
}

impl Body for ResponseBody {
    type Data = <ResponseBodyInner as Body>::Data;
    type Error = <ResponseBodyInner as Body>::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.project().inner.poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

fn simple_response(
    status_code: u16,
    headers: Vec<Header>,
    body: Bytes,
) -> http::Response<ResponseBody> {
    let mut response_builder = http::Response::builder()
        .status(status_code)
        .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE);

    for header in headers {
        response_builder = response_builder.header(header.key.deref(), header.value.deref());
    }

    response_builder
        .body(Either::Left(Full::new(body)).into())
        .expect("headers must be valid")
}

fn error_response(e: impl Into<Error>) -> http::Response<ResponseBody> {
    let error = e.into();
    http::Response::builder()
        .status(error.status_code())
        .header(X_RESTATE_SERVER, X_RESTATE_SERVER_VALUE)
        .header(CONTENT_TYPE, "text/plain")
        .body(Either::Left(Full::new(error.to_string().into())).into())
        .expect("headers must be valid")
}

// --- Handle invocation future

struct OutputSender(mpsc::UnboundedSender<Bytes>);

impl OutputSender {
    fn from_channel(tx: mpsc::UnboundedSender<Bytes>) -> Self {
        Self(tx)
    }

    fn send(&self, b: Bytes) -> bool {
        self.0.send(b).is_ok()
    }
}

struct InputReceiver(Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send + 'static>>);

impl InputReceiver {
    fn from_stream<S: Stream<Item = Result<Bytes, BoxError>> + Send + 'static>(s: S) -> Self {
        Self(Box::pin(s))
    }

    async fn recv(&mut self) -> Option<Result<Bytes, BoxError>> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, BoxError>>> {
        self.0.poll_next_unpin(cx)
    }
}

async fn handle_invocation(
    svc_name: String,
    handler_name: String,
    mut vm: CoreVM,
    endpoint: Arc<EndpointInner>,
    mut input_rx: InputReceiver,
    output_tx: OutputSender,
) -> Result<(), Error> {
    // Retrieve the service from the Arc
    let svc = endpoint
        .svcs
        .get(&svc_name)
        .expect("service must exist at this point");

    let span = info_span!(
        "restate_sdk_endpoint_handle",
        "rpc.system" = "restate",
        "rpc.service" = svc_name,
        "rpc.method" = handler_name,
        "restate.sdk.is_replaying" = false
    );
    async move {
        init_loop_vm(&mut vm, &mut input_rx).await?;

        // Initialize handler context
        let (handler_state_tx, handler_state_rx) = HandlerStateNotifier::new();
        let ctx = ContextInternal::new(
            vm,
            svc_name,
            handler_name,
            input_rx,
            output_tx,
            handler_state_tx,
        );

        // Start user code
        let user_code_fut = InterceptErrorFuture::new(ctx.clone(), svc.handle(ctx.clone()));

        // Wrap it in handler state aware future
        HandlerStateAwareFuture::new(ctx.clone(), handler_state_rx, user_code_fut).await
    }
    .instrument(span)
    .await
}

async fn init_loop_vm(vm: &mut CoreVM, input_rx: &mut InputReceiver) -> Result<(), ErrorInner> {
    while !vm.is_ready_to_execute().map_err(ErrorInner::VM)? {
        match input_rx.recv().await {
            Some(Ok(b)) => vm.notify_input(b),
            Some(Err(e)) => vm.notify_error(
                CoreError::new(500u16, format!("Error when reading the body: {e}")),
                None,
            ),
            None => vm.notify_input_closed(),
        }
    }
    Ok(())
}

// --- Invocation runner body

pub struct InvocationRunnerBody {
    fut: Option<BoxFuture<'static, Result<(), Error>>>,
    output_rx: mpsc::UnboundedReceiver<Bytes>,
    end_stream: bool,
}

impl Body for InvocationRunnerBody {
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
