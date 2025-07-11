mod context;
mod futures;
mod handler_state;

use crate::endpoint::futures::handler_state_aware::HandlerStateAwareFuture;
use crate::endpoint::futures::intercept_error::InterceptErrorFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::service::{Discoverable, Service};
use ::futures::future::BoxFuture;
use ::futures::{Stream, StreamExt};
use bytes::Bytes;
pub use context::{ContextInternal, InputMetadata};
use restate_sdk_shared_core::{
    CoreVM, Error as CoreError, Header, HeaderMap, IdentityVerifier, KeyError, VerifyError, VM,
};
use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::{info_span, Instrument};

const DISCOVERY_CONTENT_TYPE_V2: &str = "application/vnd.restate.endpointmanifest.v2+json";
const DISCOVERY_CONTENT_TYPE_V3: &str = "application/vnd.restate.endpointmanifest.v3+json";

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub struct OutputSender(tokio::sync::mpsc::UnboundedSender<Bytes>);

impl OutputSender {
    pub fn from_channel(tx: tokio::sync::mpsc::UnboundedSender<Bytes>) -> Self {
        Self(tx)
    }

    fn send(&self, b: Bytes) -> bool {
        self.0.send(b).is_ok()
    }
}

pub struct InputReceiver(InputReceiverInner);

enum InputReceiverInner {
    Channel(tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, BoxError>>),
    BoxedStream(Pin<Box<dyn Stream<Item = Result<Bytes, BoxError>> + Send + 'static>>),
    Bytes(Option<Bytes>),
}

impl InputReceiver {
    pub fn from_stream<S: Stream<Item = Result<Bytes, BoxError>> + Send + 'static>(s: S) -> Self {
        Self(InputReceiverInner::BoxedStream(Box::pin(s)))
    }

    pub fn from_channel(rx: tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, BoxError>>) -> Self {
        Self(InputReceiverInner::Channel(rx))
    }

    pub fn from_bytes(b: Bytes) -> Self {
        Self(InputReceiverInner::Bytes(Some(b)))
    }

    async fn recv(&mut self) -> Option<Result<Bytes, BoxError>> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, BoxError>>> {
        match &mut self.0 {
            InputReceiverInner::Channel(ch) => ch.poll_recv(cx),
            InputReceiverInner::BoxedStream(s) => s.poll_next_unpin(cx),
            InputReceiverInner::Bytes(b) => Poll::Ready(b.take().map(Ok)),
        }
    }
}

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
}

impl Error {
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

/// Various configuration options that can be provided when binding a service
#[derive(Default, Debug, Clone)]
pub struct ServiceOptions {
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) inactivity_timeout: Option<Duration>,
    pub(crate) abort_timeout: Option<Duration>,
    pub(crate) idempotency_retention: Option<Duration>,
    pub(crate) journal_retention: Option<Duration>,
    pub(crate) enable_lazy_state: Option<bool>,
    pub(crate) ingress_private: Option<bool>,
    pub(crate) handler_options: HashMap<String, HandlerOptions>,

    _priv: (),
}

impl ServiceOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// This timer guards against stalled invocations. Once it expires, Restate triggers a graceful
    /// termination by asking the invocation to suspend (which preserves intermediate progress).
    ///
    /// The abort_timeout is used to abort the invocation, in case it doesn't react to the request to
    /// suspend.
    ///
    /// This overrides the default inactivity timeout configured in the restate-server for all
    /// invocations to this service.
    pub fn inactivity_timeout(mut self, timeout: Duration) -> Self {
        self.inactivity_timeout = Some(timeout);
        self
    }

    /// This timer guards against stalled service/handler invocations that are supposed to terminate. The
    /// abort timeout is started after the inactivity_timeout has expired and the service/handler
    /// invocation has been asked to gracefully terminate. Once the timer expires, it will abort the
    /// service/handler invocation.
    ///
    /// This timer potentially *interrupts* user code. If the user code needs longer to gracefully
    /// terminate, then this value needs to be set accordingly.
    ///
    /// This overrides the default abort timeout configured in the restate-server for all invocations to
    /// this service.
    pub fn abort_timeout(mut self, timeout: Duration) -> Self {
        self.abort_timeout = Some(timeout);
        self
    }

    /// The retention duration of idempotent requests to this service.
    pub fn idempotency_retention(mut self, retention: Duration) -> Self {
        self.idempotency_retention = Some(retention);
        self
    }

    /// The journal retention. When set, this applies to all requests to all handlers of this service.
    ///
    /// In case the request has an idempotency key, the idempotency_retention caps the journal retention
    /// time.
    pub fn journal_retention(mut self, retention: Duration) -> Self {
        self.journal_retention = Some(retention);
        self
    }

    /// When set to `true`, lazy state will be enabled for all invocations to this service. This is
    /// relevant only for workflows and virtual objects.
    pub fn enable_lazy_state(mut self, enable: bool) -> Self {
        self.enable_lazy_state = Some(enable);
        self
    }

    /// When set to `true` this service, with all its handlers, cannot be invoked from the restate-server
    /// HTTP and Kafka ingress, but only from other services.
    pub fn ingress_private(mut self, private: bool) -> Self {
        self.ingress_private = Some(private);
        self
    }

    /// Custom metadata of this service definition. This metadata is shown on the Admin API when querying the service definition.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Handler-specific options.
    ///
    /// *Note*: If you provide a handler name for a non-existing handler, binding the service will *panic!*.
    pub fn handler(mut self, handler_name: impl Into<String>, options: HandlerOptions) -> Self {
        self.handler_options.insert(handler_name.into(), options);
        self
    }
}

/// Various configuration options that can be provided when binding a service handler
#[derive(Default, Debug, Clone)]
pub struct HandlerOptions {
    pub(crate) metadata: HashMap<String, String>,
    pub(crate) inactivity_timeout: Option<Duration>,
    pub(crate) abort_timeout: Option<Duration>,
    pub(crate) idempotency_retention: Option<Duration>,
    pub(crate) workflow_retention: Option<Duration>,
    pub(crate) journal_retention: Option<Duration>,
    pub(crate) ingress_private: Option<bool>,
    pub(crate) enable_lazy_state: Option<bool>,

    _priv: (),
}

impl HandlerOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Custom metadata of this handler definition. This metadata is shown on the Admin API when querying the service/handler definition.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// This timer guards against stalled invocations. Once it expires, Restate triggers a graceful
    /// termination by asking the invocation to suspend (which preserves intermediate progress).
    ///
    /// The abort_timeout is used to abort the invocation, in case it doesn't react to the request to
    /// suspend.
    ///
    /// This overrides the inactivity timeout set for the service and the default set in restate-server.
    pub fn inactivity_timeout(mut self, timeout: Duration) -> Self {
        self.inactivity_timeout = Some(timeout);
        self
    }

    /// This timer guards against stalled invocations that are supposed to terminate. The abort timeout
    /// is started after the inactivity_timeout has expired and the invocation has been asked to
    /// gracefully terminate. Once the timer expires, it will abort the invocation.
    ///
    /// This timer potentially *interrupts* user code. If the user code needs longer to gracefully
    /// terminate, then this value needs to be set accordingly.
    ///
    /// This overrides the abort timeout set for the service and the default set in restate-server.
    pub fn abort_timeout(mut self, timeout: Duration) -> Self {
        self.abort_timeout = Some(timeout);
        self
    }

    /// The retention duration of idempotent requests to this service.
    pub fn idempotency_retention(mut self, retention: Duration) -> Self {
        self.idempotency_retention = Some(retention);
        self
    }

    /// The retention duration for this workflow handler.
    pub fn workflow_retention(mut self, retention: Duration) -> Self {
        self.workflow_retention = Some(retention);
        self
    }

    /// The journal retention for invocations to this handler.
    ///
    /// In case the request has an idempotency key, the idempotency_retention caps the journal retention
    /// time.
    pub fn journal_retention(mut self, retention: Duration) -> Self {
        self.journal_retention = Some(retention);
        self
    }

    /// When set to `true` this handler cannot be invoked from the restate-server HTTP and Kafka ingress,
    /// but only from other services.
    pub fn ingress_private(mut self, private: bool) -> Self {
        self.ingress_private = Some(private);
        self
    }

    /// When set to `true`, lazy state will be enabled for all invocations to this handler. This is
    /// relevant only for workflows and virtual objects.
    pub fn enable_lazy_state(mut self, enable: bool) -> Self {
        self.enable_lazy_state = Some(enable);
        self
    }
}

/// Builder for [`Endpoint`]
pub struct Builder {
    svcs: HashMap<String, BoxedService>,
    discovery: crate::discovery::Endpoint,
    identity_verifier: IdentityVerifier,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            svcs: Default::default(),
            discovery: crate::discovery::Endpoint {
                max_protocol_version: 5,
                min_protocol_version: 5,
                protocol_mode: Some(crate::discovery::ProtocolMode::BidiStream),
                services: vec![],
            },
            identity_verifier: Default::default(),
        }
    }
}

impl Builder {
    /// Create a new builder for [`Endpoint`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a [`Service`] to this endpoint.
    ///
    /// When using the [`service`](macro@crate::service), [`object`](macro@crate::object) or [`workflow`](macro@crate::workflow) macros,
    /// you need to pass the result of the `serve` method.
    pub fn bind<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>>
            + Discoverable
            + Send
            + Sync
            + 'static,
    >(
        self,
        s: S,
    ) -> Self {
        self.bind_with_options(s, ServiceOptions::default())
    }

    /// Like [`bind`], but providing options
    pub fn bind_with_options<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>>
            + Discoverable
            + Send
            + Sync
            + 'static,
    >(
        mut self,
        s: S,
        service_options: ServiceOptions,
    ) -> Self {
        // Discover and apply options
        let mut service_metadata = S::discover();
        service_metadata.apply_options(service_options);

        let boxed_service = BoxedService::new(s);
        self.svcs
            .insert(service_metadata.name.to_string(), boxed_service);
        self.discovery.services.push(service_metadata);
        self
    }

    /// Add identity key, e.g. `publickeyv1_ChjENKeMvCtRnqG2mrBK1HmPKufgFUc98K8B3ononQvp`.
    pub fn identity_key(mut self, key: &str) -> Result<Self, KeyError> {
        self.identity_verifier = self.identity_verifier.with_key(key)?;
        Ok(self)
    }

    pub(crate) fn set_protocol_mode(mut self, mode: crate::discovery::ProtocolMode) -> Self {
        self.discovery.protocol_mode = Some(mode);
        self
    }

    /// Build the [`Endpoint`].
    pub fn build(self) -> Endpoint {
        Endpoint(Arc::new(EndpointInner {
            svcs: self.svcs,
            discovery: self.discovery,
            identity_verifier: self.identity_verifier,
        }))
    }

    #[cfg(feature = "lambda")]
    pub fn build_lambda(self) -> crate::lambda::LambdaEndpoint {
        crate::lambda::LambdaEndpoint::new(
            self.set_protocol_mode(crate::discovery::ProtocolMode::RequestResponse)
                .build(),
        )
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

pub struct EndpointInner {
    svcs: HashMap<String, BoxedService>,
    discovery: crate::discovery::Endpoint,
    identity_verifier: IdentityVerifier,
}

impl Endpoint {
    pub fn resolve<H>(&self, path: &str, headers: H) -> Result<Response, Error>
    where
        H: HeaderMap,
        <H as HeaderMap>::Error: std::error::Error + Send + Sync + 'static,
    {
        if let Err(e) = self.0.identity_verifier.verify_identity(&headers, path) {
            return Err(ErrorInner::IdentityVerification(e).into());
        }

        let parts: Vec<&str> = path.split('/').collect();

        if parts.last() == Some(&"health") {
            return Ok(Response::ReplyNow {
                status_code: 200,
                headers: vec![],
                body: Bytes::new(),
            });
        }

        if parts.last() == Some(&"discover") {
            // Extract Accept header from request
            let accept_header = headers
                .extract("accept")
                .map_err(|e| ErrorInner::Header("accept".to_owned(), Box::new(e)))?;

            // Negotiate discovery protocol version
            let mut version = 2;
            let mut content_type = DISCOVERY_CONTENT_TYPE_V2;
            if let Some(accept) = accept_header {
                if accept.contains(DISCOVERY_CONTENT_TYPE_V3) {
                    version = 3;
                    content_type = DISCOVERY_CONTENT_TYPE_V3;
                } else if accept.contains(DISCOVERY_CONTENT_TYPE_V2) {
                    version = 2;
                    content_type = DISCOVERY_CONTENT_TYPE_V2;
                } else {
                    Err(ErrorInner::BadDiscoveryVersion(accept.to_owned()))?
                }
            }

            // Validate that new discovery fields aren't used with older protocol versions
            if version <= 2 {
                // Check for new discovery fields in version 3 that shouldn't be used in version 2 or lower
                for service in &self.0.discovery.services {
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

            return Ok(Response::ReplyNow {
                status_code: 200,
                headers: vec![Header {
                    key: "content-type".into(),
                    value: content_type.into(),
                }],
                body: Bytes::from(
                    serde_json::to_string(&self.0.discovery)
                        .expect("Discovery should be serializable"),
                ),
            });
        }

        let (svc_name, handler_name) = match parts.get(parts.len() - 3..) {
            None => return Err(Error(ErrorInner::BadPath(path.to_owned()))),
            Some(last_elements) if last_elements[0] != "invoke" => {
                return Err(Error(ErrorInner::BadPath(path.to_owned())))
            }
            Some(last_elements) => (last_elements[1].to_owned(), last_elements[2].to_owned()),
        };

        let vm = CoreVM::new(headers, Default::default()).map_err(ErrorInner::VM)?;
        if !self.0.svcs.contains_key(&svc_name) {
            return Err(ErrorInner::UnknownService(svc_name.to_owned()).into());
        }

        let response_head = vm.get_response_head();

        Ok(Response::BidiStream {
            status_code: response_head.status_code,
            headers: response_head.headers,
            handler: BidiStreamRunner {
                svc_name,
                handler_name,
                vm,
                endpoint: Arc::clone(&self.0),
            },
        })
    }
}

pub enum Response {
    ReplyNow {
        status_code: u16,
        headers: Vec<Header>,
        body: Bytes,
    },
    BidiStream {
        status_code: u16,
        headers: Vec<Header>,
        handler: BidiStreamRunner,
    },
}

pub struct BidiStreamRunner {
    svc_name: String,
    handler_name: String,
    vm: CoreVM,
    endpoint: Arc<EndpointInner>,
}

impl BidiStreamRunner {
    pub async fn handle(
        self,
        input_rx: InputReceiver,
        output_tx: OutputSender,
    ) -> Result<(), Error> {
        // Retrieve the service from the Arc
        let svc = self
            .endpoint
            .svcs
            .get(&self.svc_name)
            .expect("service must exist at this point");

        let span = info_span!(
            "restate_sdk_endpoint_handle",
            "rpc.system" = "restate",
            "rpc.service" = self.svc_name,
            "rpc.method" = self.handler_name,
            "restate.sdk.is_replaying" = false
        );
        handle(
            input_rx,
            output_tx,
            self.vm,
            self.svc_name,
            self.handler_name,
            svc,
        )
        .instrument(span)
        .await
    }
}

#[doc(hidden)]
pub async fn handle<S: Service<Future = BoxFuture<'static, Result<(), Error>>> + Send + Sync>(
    mut input_rx: InputReceiver,
    output_tx: OutputSender,
    vm: CoreVM,
    svc_name: String,
    handler_name: String,
    svc: &S,
) -> Result<(), Error> {
    let mut vm = vm;
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
