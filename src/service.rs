use crate::endpoint;
use crate::endpoint::{BoxedService, ContextInternal};
use crate::extensions::ExtensionMap;
use futures::future::BoxFuture;
use std::any::Any;
use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;

/// Trait representing a Restate service.
///
/// This is the low-level runtime trait the SDK dispatches to. It is implemented by codegen
/// (the [`service`](crate::service()) builder and the deprecated trait macros) and can be
/// implemented by hand for advanced use cases (see [`ServiceDefinition::from_service`]).
pub trait Service {
    type Future: Future<Output = Result<(), endpoint::Error>> + Send + 'static;

    /// Handle an incoming request.
    fn handle(&self, req: endpoint::ContextInternal) -> Self::Future;
}

/// Trait representing a discoverable Restate service.
///
/// This is used by codegen.
pub trait Discoverable {
    fn discover() -> crate::discovery::Service;
}

/// Used by codegen
#[doc(hidden)]
pub type ServiceBoxFuture = BoxFuture<'static, Result<(), endpoint::Error>>;

// ============================ Handler abstraction ============================

/// Marker for plain [Services](https://docs.restate.dev/concepts/services#services-1). Used so that
/// handlers of one kind cannot be composed into a service of another kind.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServiceKind;
/// Marker for [Virtual Objects](https://docs.restate.dev/concepts/services#virtual-objects).
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjectKind;
/// Marker for [Workflows](https://docs.restate.dev/concepts/services#workflows).
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkflowKind;

/// Static metadata describing a single handler, used to build the discovery manifest.
#[derive(Clone)]
pub struct HandlerMeta {
    /// The Restate name of the handler.
    pub name: Cow<'static, str>,
    /// The handler type, or `None` to use the discovery manifest default (exclusive).
    pub ty: Option<crate::discovery::HandlerType>,
    /// Input payload schema.
    pub input: crate::discovery::InputPayload,
    /// Output payload schema.
    pub output: crate::discovery::OutputPayload,
    /// Whether lazy state is enabled for this handler.
    pub enable_lazy_state: Option<bool>,
}

impl HandlerMeta {
    fn into_discovery(self) -> crate::discovery::Handler {
        crate::discovery::Handler {
            name: crate::discovery::HandlerName::try_from(self.name.into_owned())
                .expect("Handler name valid"),
            input: Some(self.input),
            output: Some(self.output),
            ty: self.ty,
            documentation: None,
            metadata: Default::default(),
            abort_timeout: None,
            inactivity_timeout: None,
            journal_retention: None,
            idempotency_retention: None,
            workflow_completion_retention: None,
            enable_lazy_state: self.enable_lazy_state,
            ingress_private: None,
            retry_policy_initial_interval: None,
            retry_policy_max_interval: None,
            retry_policy_max_attempts: None,
            retry_policy_exponentiation_factor: None,
            retry_policy_on_max_attempts: None,
        }
    }
}

/// A single Restate handler, as a composable value.
///
/// This is the "well-defined trait" that codegen (the [`macro@crate::handler`] attribute macro)
/// implements for you. `Kind` is a compile-time marker ([`ServiceKind`], [`ObjectKind`],
/// [`WorkflowKind`]) that prevents mixing handlers of different service kinds in one builder.
pub trait Handler<Kind>: Send + Sync + 'static {
    /// Static metadata for discovery.
    fn meta(&self) -> HandlerMeta;

    /// Handle an incoming invocation. Takes ownership of the [`ContextInternal`] and builds the
    /// user-facing context borrow internally.
    fn handle(&self, ctx: ContextInternal) -> ServiceBoxFuture;
}

/// A [`Handler`] that additionally exposes its input/output types, used for compile-time
/// conformance checking by [`macro@crate::interface`].
pub trait TypedHandler<Kind>: Handler<Kind> {
    type Input;
    type Output;
}

/// Codegen support: wraps a handler to override its discovery/dispatch name.
///
/// Used by [`macro@crate::interface`] so that a handler bound to an interface slot is always
/// registered under the interface's declared name (keeping the generated client and server in sync).
#[doc(hidden)]
pub struct Named<H> {
    pub handler: H,
    pub name: &'static str,
}

impl<Kind, H: Handler<Kind>> Handler<Kind> for Named<H> {
    fn meta(&self) -> HandlerMeta {
        HandlerMeta {
            name: Cow::Borrowed(self.name),
            ..self.handler.meta()
        }
    }

    fn handle(&self, ctx: ContextInternal) -> ServiceBoxFuture {
        self.handler.handle(ctx)
    }
}

// ============================ Composition ============================

/// Internal dispatcher: routes an invocation to the right handler by name.
struct HandlerMap<Kind> {
    handlers: HashMap<String, Box<dyn Handler<Kind>>>,
}

impl<Kind: 'static> Service for HandlerMap<Kind> {
    type Future = ServiceBoxFuture;

    fn handle(&self, ctx: ContextInternal) -> Self::Future {
        match self.handlers.get(ctx.handler_name()) {
            Some(handler) => handler.handle(ctx),
            None => {
                let err = endpoint::Error::unknown_handler(ctx.service_name(), ctx.handler_name());
                Box::pin(async move { Err(err) })
            }
        }
    }
}

/// Builder to compose [`Handler`]s into a bindable [`ServiceDefinition`].
///
/// Obtain one via [`service()`], [`object()`] or [`workflow()`].
pub struct ServiceBuilder<Kind> {
    name: String,
    service_type: crate::discovery::ServiceType,
    handlers: Vec<Box<dyn Handler<Kind>>>,
    extensions: ExtensionMap,
}

impl<Kind: 'static> ServiceBuilder<Kind> {
    fn new(name: String, service_type: crate::discovery::ServiceType) -> Self {
        Self {
            name,
            service_type,
            handlers: Vec::new(),
            extensions: ExtensionMap::new(),
        }
    }

    /// Add a handler to this service.
    pub fn handler(mut self, handler: impl Handler<Kind>) -> Self {
        self.handlers.push(Box::new(handler));
        self
    }

    /// Register a service-scoped dependency (extension), retrievable inside handlers via
    /// [`ContextExtensions::extension`](crate::context::ContextExtensions::extension). Overrides any
    /// endpoint-level extension of the same type.
    pub fn extension<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// Finalize into a [`ServiceDefinition`] to pass to
    /// [`Endpoint::builder().bind(..)`](crate::endpoint::Builder::bind).
    pub fn build(self) -> ServiceDefinition {
        let mut dispatch: HashMap<String, Box<dyn Handler<Kind>>> =
            HashMap::with_capacity(self.handlers.len());
        let mut discovery_handlers = Vec::with_capacity(self.handlers.len());
        for handler in self.handlers {
            let meta = handler.meta();
            let name = meta.name.clone().into_owned();
            discovery_handlers.push(meta.into_discovery());
            dispatch.insert(name, handler);
        }

        let discovery = crate::discovery::Service {
            ty: self.service_type,
            name: crate::discovery::ServiceName::try_from(self.name).expect("Service name valid"),
            handlers: discovery_handlers,
            documentation: None,
            metadata: Default::default(),
            abort_timeout: None,
            inactivity_timeout: None,
            journal_retention: None,
            idempotency_retention: None,
            enable_lazy_state: None,
            ingress_private: None,
            retry_policy_initial_interval: None,
            retry_policy_max_interval: None,
            retry_policy_max_attempts: None,
            retry_policy_exponentiation_factor: None,
            retry_policy_on_max_attempts: None,
        };

        ServiceDefinition {
            discovery,
            dispatcher: BoxedService::new(HandlerMap { handlers: dispatch }),
            extensions: self.extensions,
        }
    }
}

/// Start building a plain [Service](https://docs.restate.dev/concepts/services#services-1).
pub fn service(name: impl Into<String>) -> ServiceBuilder<ServiceKind> {
    ServiceBuilder::new(name.into(), crate::discovery::ServiceType::Service)
}

/// Start building a [Virtual Object](https://docs.restate.dev/concepts/services#virtual-objects).
pub fn object(name: impl Into<String>) -> ServiceBuilder<ObjectKind> {
    ServiceBuilder::new(name.into(), crate::discovery::ServiceType::VirtualObject)
}

/// Start building a [Workflow](https://docs.restate.dev/concepts/services#workflows).
pub fn workflow(name: impl Into<String>) -> ServiceBuilder<WorkflowKind> {
    ServiceBuilder::new(name.into(), crate::discovery::ServiceType::Workflow)
}

// ============================ ServiceDefinition ============================

/// A fully-composed service, ready to be bound to an [`Endpoint`](crate::endpoint::Endpoint).
///
/// Produced by [`ServiceBuilder::build`], and consumed by
/// [`Endpoint::builder().bind(..)`](crate::endpoint::Builder::bind).
pub struct ServiceDefinition {
    pub(crate) discovery: crate::discovery::Service,
    pub(crate) dispatcher: BoxedService,
    pub(crate) extensions: ExtensionMap,
}

impl ServiceDefinition {
    /// Build a definition from a hand-rolled [`Service`] + [`Discoverable`] implementation.
    ///
    /// This is the escape hatch used for back-compat with the deprecated trait macros.
    pub fn from_service<S>(s: S) -> Self
    where
        S: Service<Future = ServiceBoxFuture> + Discoverable + Send + Sync + 'static,
    {
        Self {
            discovery: S::discover(),
            dispatcher: BoxedService::new(s),
            extensions: ExtensionMap::new(),
        }
    }
}

/// Anything that can be turned into a [`ServiceDefinition`] for
/// [`Endpoint::builder().bind(..)`](crate::endpoint::Builder::bind).
///
/// Implemented for [`ServiceDefinition`] itself (the new builder output) and, via a blanket impl,
/// for any [`Service`] + [`Discoverable`] (the deprecated `.serve()` values).
pub trait IntoServiceDefinition {
    fn into_service_definition(self) -> ServiceDefinition;
}

impl IntoServiceDefinition for ServiceDefinition {
    fn into_service_definition(self) -> ServiceDefinition {
        self
    }
}

impl<S> IntoServiceDefinition for S
where
    S: Service<Future = ServiceBoxFuture> + Discoverable + Send + Sync + 'static,
{
    fn into_service_definition(self) -> ServiceDefinition {
        ServiceDefinition::from_service(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ServiceType;

    // Hand-written handler, mirroring what `#[restate::handler]` will generate.
    struct Greet;
    impl Handler<ServiceKind> for Greet {
        fn meta(&self) -> HandlerMeta {
            HandlerMeta {
                name: "greet".into(),
                ty: None,
                input: crate::discovery::InputPayload::empty(),
                output: crate::discovery::OutputPayload::empty(),
                enable_lazy_state: None,
            }
        }
        fn handle(&self, _ctx: ContextInternal) -> ServiceBoxFuture {
            Box::pin(async { Ok(()) })
        }
    }
    impl TypedHandler<ServiceKind> for Greet {
        type Input = String;
        type Output = String;
    }

    // Proves the conformance-slot bound compiles: this is what `interface!` will generate for
    // each declared handler, guaranteeing the impl's input/output types match the interface.
    fn slot<H: TypedHandler<ServiceKind, Input = String, Output = String>>(_h: H) {}

    #[test]
    fn builds_definition_with_discovery_and_state() {
        let def = service("Greeter").handler(Greet).extension(42u32).build();

        assert!(matches!(def.discovery.ty, ServiceType::Service));
        assert_eq!(def.discovery.name.to_string(), "Greeter");
        assert_eq!(def.discovery.handlers.len(), 1);
        assert_eq!(def.discovery.handlers[0].name.as_str(), "greet");

        // Ambient (service-scoped) extension was recorded.
        assert_eq!(def.extensions.get::<u32>(), Some(&42));

        slot(Greet); // conformance bound holds for a matching handler
    }
}
