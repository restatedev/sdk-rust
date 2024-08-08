mod handler_context;
mod handler_state;

use crate::endpoint::handler_state::HandlerStateNotifier;
use crate::service::{Discoverable, Service};
use bytes::Bytes;
use futures::future::BoxFuture;
pub use handler_context::HandlerContext;
use restate_sdk_shared_core::{CoreVM, HeaderMap, VMError, VM};
use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::sync::Arc;
use std::task::{Context, Poll};

struct OutputSender(tokio::sync::mpsc::UnboundedSender<Bytes>);

impl OutputSender {
    fn send(&self, b: Bytes) -> bool {
        self.0.send(b).is_ok()
    }
}

struct InputReceiver(tokio::sync::mpsc::UnboundedReceiver<Bytes>);

impl InputReceiver {
    async fn recv(&mut self) -> Option<Bytes> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Bytes>> {
        self.0.poll_recv(cx)
    }
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
struct Error(#[from] ErrorInner);

impl Error {
    fn code(&self) -> u16 {
        match &self.0 {
            ErrorInner::UnknownService(_) => 404,
            ErrorInner::VM(e) => e.code,
            ErrorInner::Suspended => 500,
            ErrorInner::UnexpectedOutputClosed => 500,
            ErrorInner::UnexpectedValueVariantForSyscall { .. } => 500,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ErrorInner {
    #[error("Received a request for unkonwn service '{0}'")]
    UnknownService(String),
    #[error("Error when processing the request: {0:?}")]
    VM(#[from] VMError),
    #[error("Suspended")]
    Suspended,
    #[error("Unexpected output closed")]
    UnexpectedOutputClosed,
    #[error("Unexpected value variant {variant} for syscall '{syscall}'")]
    UnexpectedValueVariantForSyscall {
        variant: &'static str,
        syscall: &'static str,
    },
}

struct BoxedService(Box<dyn Service<Future = BoxFuture<'static, ()>> + Send + Sync + 'static>);

impl BoxedService {
    pub fn new<S: Service<Future = BoxFuture<'static, ()>> + Send + Sync + 'static>(
        service: S,
    ) -> Self {
        Self(Box::new(service))
    }
}

impl Service for BoxedService {
    type Future = BoxFuture<'static, ()>;

    fn handle(&self, req: HandlerContext) -> Self::Future {
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

    pub  fn with_service<
        S: Service<Future = BoxFuture<'static, ()>> + Discoverable + Send + Sync + 'static,
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

pub struct Endpoint(Arc<EndpointInner>);

pub struct EndpointInner {
    svcs: HashMap<String, BoxedService>,
    discovery: crate::discovery::Endpoint,
}

impl Endpoint {
    fn handle(
        &self,
        svc_name: &str,
        handler_name: &str,
        headers: impl HeaderMap,
        mut input_rx: InputReceiver,
        output_tx: OutputSender,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'static {
        let vm = CoreVM::new(headers);

        let this = Arc::clone(&self.0);
        let svc_name = svc_name.to_owned();
        let handler_name = handler_name.to_owned();
        return async move {
            let svc = this.svcs.get(&svc_name);
            let svc = svc.ok_or_else(|| ErrorInner::UnknownService(svc_name.to_owned()))?;

            let mut vm = vm.map_err(ErrorInner::VM)?;
            Self::init_loop_vm(&mut vm, &mut input_rx).await?;

            let (handler_state_tx, handler_state_rx) = HandlerStateNotifier::new();

            let handler_context = HandlerContext::new(vm, svc_name, handler_name, input_rx, output_tx, handler_state_tx);

            return handler_state::handler_state_aware_future(
                handler_state_rx,
                svc.handle(handler_context),
            )
            .await;
        };
    }

    async fn init_loop_vm(vm: &mut CoreVM, input_rx: &mut InputReceiver) -> Result<(), ErrorInner> {
        while !vm.is_ready_to_execute().map_err(ErrorInner::VM)? {
            if let Some(b) = input_rx.recv().await {
                vm.notify_input(b.to_vec())
            } else {
                vm.notify_input_closed();
            }
        }
        Ok(())
    }
}
