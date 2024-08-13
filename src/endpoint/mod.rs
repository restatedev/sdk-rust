mod context;
mod futures;
mod handler_state;

use crate::endpoint::futures::InterceptErrorFuture;
use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::service::{Discoverable, Service};
use ::futures::future::BoxFuture;
use ::futures::{Stream, StreamExt};
use bytes::Bytes;
pub use context::{ContextInternal, InputMetadata};
use restate_sdk_shared_core::{CoreVM, Header, HeaderMap, ResponseHead, VMError, VM};
use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

const DISCOVERY_CONTENT_TYPE: &str = "application/vnd.restate.endpointmanifest.v1+json";

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
}

impl InputReceiver {
    pub fn from_stream<S: Stream<Item = Result<Bytes, BoxError>> + Send + 'static>(s: S) -> Self {
        Self(InputReceiverInner::BoxedStream(Box::pin(s)))
    }

    pub fn from_channel(rx: tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, BoxError>>) -> Self {
        Self(InputReceiverInner::Channel(rx))
    }

    async fn recv(&mut self) -> Option<Result<Bytes, BoxError>> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Bytes, BoxError>>> {
        match &mut self.0 {
            InputReceiverInner::Channel(ch) => ch.poll_recv(cx),
            InputReceiverInner::BoxedStream(s) => s.poll_next_unpin(cx),
        }
    }
}

// TODO can we have the backtrace here?
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(#[from] ErrorInner);

impl Error {
    pub fn unknown_handler(service_name: &str, handler_name: &str) -> Self {
        Self(ErrorInner::UnknownServiceHandler(
            service_name.to_owned(),
            handler_name.to_owned(),
        ))
    }
}

impl Error {
    pub fn status_code(&self) -> u16 {
        match &self.0 {
            ErrorInner::VM(e) => e.code,
            ErrorInner::UnknownService(_) | ErrorInner::UnknownServiceHandler(_, _) => 404,
            ErrorInner::Suspended
            | ErrorInner::UnexpectedOutputClosed
            | ErrorInner::UnexpectedValueVariantForSyscall { .. }
            | ErrorInner::Deserialization { .. }
            | ErrorInner::Serialization { .. }
            | ErrorInner::RunResult { .. }
            | ErrorInner::HandlerResult { .. } => 500,
            ErrorInner::BadDiscovery(_) => 415,
            ErrorInner::Header { .. } | ErrorInner::BadPath { .. } => 400,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ErrorInner {
    #[error("Received a request for unknown service '{0}'")]
    UnknownService(String),
    #[error("Received a request for unknown service handler '{0}/{1}'")]
    UnknownServiceHandler(String, String),
    #[error("Error when processing the request: {0:?}")]
    VM(#[from] VMError),
    #[error("Cannot read header '{0}', reason: {1}")]
    Header(&'static str, #[source] BoxError),
    #[error("Cannot reply to discovery, got accept header '{0}' but currently supported discovery is {DISCOVERY_CONTENT_TYPE}")]
    BadDiscovery(String),
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
    #[error("Run '{name}' failed with retryable error: {err:?}'")]
    RunResult {
        name: String,
        #[source]
        err: BoxError,
    },
    #[error("Handler failed with retryable error: {err:?}'")]
    HandlerResult {
        #[source]
        err: BoxError,
    },
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

pub struct Builder {
    svcs: HashMap<String, BoxedService>,
    discovery: crate::discovery::Endpoint,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            svcs: Default::default(),
            discovery: crate::discovery::Endpoint {
                max_protocol_version: 1,
                min_protocol_version: 1,
                protocol_mode: Some(crate::discovery::ProtocolMode::BidiStream),
                services: vec![],
            },
        }
    }
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_service<
        S: Service<Future = BoxFuture<'static, Result<(), Error>>>
            + Discoverable
            + Send
            + Sync
            + 'static,
    >(
        mut self,
        s: S,
    ) -> Self {
        let service_metadata = S::discover();
        let boxed_service = BoxedService::new(s);
        self.svcs
            .insert(service_metadata.name.to_string(), boxed_service);
        self.discovery.services.push(service_metadata);
        self
    }

    pub fn build(self) -> Endpoint {
        Endpoint(Arc::new(EndpointInner {
            svcs: self.svcs,
            discovery: self.discovery,
        }))
    }
}

#[derive(Clone)]
pub struct Endpoint(Arc<EndpointInner>);

impl Endpoint {
    pub fn builder() -> Builder {
        Builder::new()
    }
}

pub struct EndpointInner {
    svcs: HashMap<String, BoxedService>,
    discovery: crate::discovery::Endpoint,
}

impl Endpoint {
    pub fn resolve<H>(&self, path: &str, headers: H) -> Result<Response, Error>
    where
        H: HeaderMap,
        <H as HeaderMap>::Error: std::error::Error + Send + Sync + 'static,
    {
        let parts: Vec<&str> = path.split('/').collect();

        if parts.last() == Some(&"discover") {
            let accept_header = headers
                .extract("accept")
                .map_err(|e| ErrorInner::Header("accept", Box::new(e)))?;
            if accept_header.is_some() {
                let accept = accept_header.unwrap();
                if !accept.contains("application/vnd.restate.endpointmanifest.v1+json") {
                    return Err(Error(ErrorInner::BadDiscovery(accept.to_owned())));
                }
            }

            return Ok(Response::ReplyNow {
                response_head: ResponseHead {
                    status_code: 200,
                    headers: vec![Header {
                        key: "content-type".into(),
                        value: DISCOVERY_CONTENT_TYPE.into(),
                    }],
                },
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

        let vm = CoreVM::new(headers).map_err(ErrorInner::VM)?;
        if !self.0.svcs.contains_key(&svc_name) {
            return Err(ErrorInner::UnknownService(svc_name.to_owned()).into());
        }

        Ok(Response::BidiStream {
            response_head: vm.get_response_head(),
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
        response_head: ResponseHead,
        body: Bytes,
    },
    BidiStream {
        response_head: ResponseHead,
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
        mut self,
        mut input_rx: InputReceiver,
        output_tx: OutputSender,
    ) -> Result<(), Error> {
        Self::init_loop_vm(&mut self.vm, &mut input_rx).await?;

        // Retrieve the service from the Arc
        let svc = self
            .endpoint
            .svcs
            .get(&self.svc_name)
            .expect("service must exist at this point");

        // Initialize handler context
        let (handler_state_tx, handler_state_rx) = HandlerStateNotifier::new();
        let ctx = ContextInternal::new(
            self.vm,
            self.svc_name,
            self.handler_name,
            input_rx,
            output_tx,
            handler_state_tx,
        );

        // Start user code
        let user_code_fut = InterceptErrorFuture::new(ctx.clone(), svc.handle(ctx.clone()));

        // Wrap it in handler state aware future
        futures::HandlerStateAwareFuture::new(ctx.clone(), handler_state_rx, user_code_fut).await
    }

    async fn init_loop_vm(vm: &mut CoreVM, input_rx: &mut InputReceiver) -> Result<(), ErrorInner> {
        while !vm.is_ready_to_execute().map_err(ErrorInner::VM)? {
            match input_rx.recv().await {
                Some(Ok(b)) => vm.notify_input(b.to_vec()),
                Some(Err(e)) => {
                    vm.notify_error("Error when reading the body".into(), e.to_string().into())
                }
                None => vm.notify_input_closed(),
            }
        }
        Ok(())
    }
}
