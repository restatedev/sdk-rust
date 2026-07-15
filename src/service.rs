use crate::endpoint;
use crate::endpoint::{BoxedService, ContextInternal};
use crate::extensions::ExtensionMap;
use futures::future::BoxFuture;
use std::any::Any;
use std::borrow::Cow;
use std::future::Future;

/// Trait representing a Restate service.
///
/// This is the low-level runtime trait the SDK dispatches to. It is implemented by codegen (the
/// `service!`/`object!`/`workflow!` macros and the deprecated trait macros) and can be implemented
/// by hand for advanced use cases (see [`ServiceDefinition::from_service`]).
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
/// [`WorkflowKind`]) that prevents mixing handlers of different service kinds in one service. The
/// `Input`/`Output` associated types type the generated clients and back the compile-time
/// conformance check performed by [`macro@crate::interface`].
pub trait Handler<Kind>: Send + Sync + 'static {
    /// The handler's deserialized input type.
    type Input;
    /// The handler's success output type.
    type Output;

    /// Static metadata for discovery.
    fn meta(&self) -> HandlerMeta;

    /// Handle an incoming invocation. Takes ownership of the [`ContextInternal`] and builds the
    /// user-facing context borrow internally.
    fn handle(&self, ctx: ContextInternal) -> ServiceBoxFuture;
}

// ============================ ServiceDefinition ============================

/// A fully-composed service, ready to be bound to an [`Endpoint`](crate::endpoint::Endpoint).
///
/// Produced by the `service!`/`object!`/`workflow!` macros, and consumed by
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

    /// Register a service-scoped dependency (extension), retrievable inside handlers via
    /// [`ContextExtensions::extension`](crate::context::ContextExtensions::extension).
    ///
    /// Overrides any endpoint-level extension of the same type.
    pub fn with_extension<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.extensions.insert(value);
        self
    }

    /// Apply [`ServiceOptions`](crate::endpoint::ServiceOptions) (timeouts, retention, per-handler
    /// options, ...) to this definition.
    pub fn with_options(mut self, options: crate::endpoint::ServiceOptions) -> Self {
        self.discovery.apply_options(options);
        self
    }
}

/// Anything that can be turned into a [`ServiceDefinition`] for
/// [`Endpoint::builder().bind(..)`](crate::endpoint::Builder::bind).
///
/// Implemented for [`ServiceDefinition`], for the service types generated by the
/// `service!`/`object!`/`workflow!` macros, and, via a blanket impl, for any [`Service`] +
/// [`Discoverable`] (the deprecated `.serve()` values).
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

/// Codegen support used by the `service!`/`object!`/`workflow!` macros.
///
/// Not part of the public API.
#[doc(hidden)]
pub mod macro_support {
    use super::*;

    /// Build the discovery manifest for a service from its handlers' [`HandlerMeta`]. Backs the
    /// `Discoverable` impl the `service!`/`object!`/`workflow!` macros generate; the dispatch
    /// `match` itself is hardcoded in the generated [`Service`] impl.
    pub fn build_discovery(
        name: &str,
        service_type: crate::discovery::ServiceType,
        handlers: Vec<HandlerMeta>,
    ) -> crate::discovery::Service {
        crate::discovery::Service {
            ty: service_type,
            name: crate::discovery::ServiceName::try_from(name.to_owned())
                .expect("Service name valid"),
            handlers: handlers
                .into_iter()
                .map(HandlerMeta::into_discovery)
                .collect(),
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
        }
    }

    /// Dispatcher fallback for when an invocation names a handler this service doesn't define.
    pub fn unknown_handler(ctx: &ContextInternal) -> ServiceBoxFuture {
        let err = endpoint::Error::unknown_handler(ctx.service_name(), ctx.handler_name());
        Box::pin(async move { Err(err) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ServiceType;

    // Hand-written handler, mirroring what `#[restate::handler]` generates.
    struct Greet;
    impl Handler<ServiceKind> for Greet {
        type Input = String;
        type Output = String;
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

    // Hand-written dispatcher + discovery, mirroring what the `service!` macro generates: a
    // hardcoded name→handler `match` and a `Discoverable` impl driven by `build_discovery`.
    struct GreeterSvc;
    impl Service for GreeterSvc {
        type Future = ServiceBoxFuture;
        fn handle(&self, ctx: ContextInternal) -> Self::Future {
            if ctx.handler_name() == "greet" {
                return Greet.handle(ctx);
            }
            macro_support::unknown_handler(&ctx)
        }
    }
    impl Discoverable for GreeterSvc {
        fn discover() -> crate::discovery::Service {
            macro_support::build_discovery("Greeter", ServiceType::Service, vec![Greet.meta()])
        }
    }

    // Proves the client-typing bound compiles: the generated client reads a handler's
    // input/output through these associated types.
    fn slot<H: Handler<ServiceKind, Input = String, Output = String>>(_h: H) {}

    #[test]
    fn builds_definition_with_discovery_and_extension() {
        let def = ServiceDefinition::from_service(GreeterSvc).with_extension(42u32);

        assert!(matches!(def.discovery.ty, ServiceType::Service));
        assert_eq!(def.discovery.name.to_string(), "Greeter");
        assert_eq!(def.discovery.handlers.len(), 1);
        assert_eq!(def.discovery.handlers[0].name.as_str(), "greet");
        assert_eq!(def.extensions.get::<u32>(), Some(&42));

        slot(Greet); // input/output bound holds for a matching handler
    }
}
