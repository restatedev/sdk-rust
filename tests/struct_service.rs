//! Tests for the struct-based service API, including discovery parity with the (deprecated)
//! trait-based API.
#![allow(deprecated)]

use restate_sdk::prelude::*;
use restate_sdk::service::Discoverable;

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

struct MySvc;

#[service(name = "ParityService")]
impl MySvc {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(name)
    }

    #[handler(name = "noInput")]
    async fn no_input(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[service]
#[name = "ParityService"]
trait ParityServiceTrait {
    async fn greet(name: String) -> HandlerResult<String>;
    #[name = "noInput"]
    async fn no_input() -> HandlerResult<()>;
}

struct ParityServiceTraitImpl;

impl ParityServiceTrait for ParityServiceTraitImpl {
    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(name)
    }
    async fn no_input(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Virtual object (with a shared handler, inferred from the context type)
// ---------------------------------------------------------------------------

struct MyObj;

#[object(name = "ParityObject")]
impl MyObj {
    #[handler]
    async fn add(&self, _ctx: ObjectContext<'_>, v: u64) -> HandlerResult<u64> {
        Ok(v)
    }

    #[handler]
    async fn get(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        Ok(0)
    }
}

#[object]
#[name = "ParityObject"]
trait ParityObjectTrait {
    async fn add(v: u64) -> HandlerResult<u64>;
    #[shared]
    async fn get() -> HandlerResult<u64>;
}

struct ParityObjectTraitImpl;

impl ParityObjectTrait for ParityObjectTraitImpl {
    async fn add(&self, _ctx: ObjectContext<'_>, v: u64) -> HandlerResult<u64> {
        Ok(v)
    }
    async fn get(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<u64> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// Workflow
// ---------------------------------------------------------------------------

struct MyWf;

#[workflow(name = "ParityWorkflow")]
impl MyWf {
    #[handler]
    async fn run(&self, _ctx: WorkflowContext<'_>, req: String) -> HandlerResult<String> {
        Ok(req)
    }

    #[handler]
    async fn signal(&self, _ctx: SharedWorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[workflow]
#[name = "ParityWorkflow"]
trait ParityWorkflowTrait {
    async fn run(req: String) -> HandlerResult<String>;
    #[shared]
    async fn signal() -> HandlerResult<()>;
}

struct ParityWorkflowTraitImpl;

impl ParityWorkflowTrait for ParityWorkflowTraitImpl {
    async fn run(&self, _ctx: WorkflowContext<'_>, req: String) -> HandlerResult<String> {
        Ok(req)
    }
    async fn signal(&self, _ctx: SharedWorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn assert_discovery_eq(a: &restate_sdk::discovery::Service, b: &restate_sdk::discovery::Service) {
    assert_eq!(
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap(),
        "struct-API discovery must match the trait-API discovery"
    );
}

#[test]
fn service_discovery_matches_trait() {
    let struct_disc = <MySvc as Discoverable>::discover();
    let trait_disc = ServeParityServiceTrait::<ParityServiceTraitImpl>::discover();
    assert_eq!(struct_disc.name.to_string(), "ParityService");
    assert_discovery_eq(&struct_disc, &trait_disc);
}

#[test]
fn object_discovery_matches_trait() {
    let struct_disc = <MyObj as Discoverable>::discover();
    let trait_disc = ServeParityObjectTrait::<ParityObjectTraitImpl>::discover();
    assert_discovery_eq(&struct_disc, &trait_disc);
}

#[test]
fn workflow_discovery_matches_trait() {
    let struct_disc = <MyWf as Discoverable>::discover();
    let trait_disc = ServeParityWorkflowTrait::<ParityWorkflowTraitImpl>::discover();
    assert_discovery_eq(&struct_disc, &trait_disc);
}

// ---------------------------------------------------------------------------
// Generic services (generic parameter used for dependency injection; concrete wire types)
// ---------------------------------------------------------------------------

trait Greeting: Send + Sync + 'static {
    fn greeting(&self) -> String;
}

#[derive(Clone)]
struct English;
impl Greeting for English {
    fn greeting(&self) -> String {
        "Hello".to_string()
    }
}

struct GenericGreeter<G> {
    with: G,
}

#[service(name = "GenericGreeter")]
impl<G: Greeting> GenericGreeter<G> {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>, name: String) -> HandlerResult<String> {
        Ok(format!("{} {name}", self.with.greeting()))
    }
}

// Generic with an explicit where-clause and a lifetime bound.
struct BoundedObject<G> {
    with: G,
}

#[object(name = "BoundedObject")]
impl<G> BoundedObject<G>
where
    G: Greeting + 'static,
{
    #[handler]
    async fn hi(&self, _ctx: SharedObjectContext<'_>) -> HandlerResult<String> {
        Ok(self.with.greeting())
    }
}

#[test]
fn generic_service_discovers_and_binds() {
    let disc = <GenericGreeter<English> as Discoverable>::discover();
    assert_eq!(disc.name.to_string(), "GenericGreeter");
    assert_eq!(disc.handlers.len(), 1);

    let _ = Endpoint::builder()
        .bind(GenericGreeter { with: English })
        .bind(BoundedObject { with: English })
        .build();
}

// `client_visibility` overrides the generated client's visibility.
struct Restricted;

#[service(name = "Restricted", client_visibility = "pub(crate)")]
impl Restricted {
    #[handler]
    async fn ping(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn client_visibility_override_discovers() {
    let disc = <Restricted as Discoverable>::discover();
    assert_eq!(disc.name.to_string(), "Restricted");
    // `RestrictedClient` is generated with `pub(crate)` visibility.
    let _bind = Endpoint::builder().bind(Restricted).build();
}

#[test]
fn binds_without_serve() {
    // The struct value binds directly, no `.serve()`.
    let _ = Endpoint::builder()
        .bind(MySvc)
        .bind(MyObj)
        .bind(MyWf)
        .build();
}

// ---------------------------------------------------------------------------
// Generated ingress clients
// ---------------------------------------------------------------------------

/// Deliberately has the same name as the ingress generator's preferred executor type parameter.
/// The generated parameter must be made fresh rather than shadowing this service generic.
#[allow(dead_code)]
struct GenericIngressNames<'a, r#__RestateIngressExecutor> {
    _marker: std::marker::PhantomData<&'a r#__RestateIngressExecutor>,
}

#[service(name = "RenamedGenericIngress", client_visibility = "pub(crate)")]
impl<'a, r#__RestateIngressExecutor> GenericIngressNames<'a, r#__RestateIngressExecutor>
where
    'a: 'static,
    r#__RestateIngressExecutor: Greeting,
{
    // `new` remains a valid handler name because ingress clients use `from_client` as their
    // constructor. The configured Restate handler name is independent of the Rust method name.
    #[handler(name = "renamedNew")]
    #[allow(dead_code, clippy::wrong_self_convention, clippy::new_ret_no_self)]
    async fn new(&self, _ctx: Context<'_>, value: String) -> HandlerResult<String> {
        Ok(value)
    }
}

struct MacroTestExecutor;

impl restate_sdk::ingress::RequestExecutor for MacroTestExecutor {
    type Error = std::convert::Infallible;

    async fn execute(
        &self,
        _request: http::Request<bytes::Bytes>,
    ) -> Result<http::Response<bytes::Bytes>, Self::Error> {
        Ok(http::Response::new(bytes::Bytes::new()))
    }
}

fn macro_test_ingress_client() -> restate_sdk::ingress::Client<MacroTestExecutor> {
    restate_sdk::ingress::Client::new("http://localhost:8080".parse().unwrap(), MacroTestExecutor)
        .unwrap()
}

#[test]
fn generated_ingress_clients_have_typed_natural_apis() {
    // `MacroTestExecutor` intentionally does not implement Clone: cloning the base client must not
    // impose that bound, and every generated client owns its clone.
    let client = macro_test_ingress_client();

    let service = MySvcIngressClient::from_client(client.clone());
    let _: restate_sdk::ingress::Request<MacroTestExecutor, String, String> =
        service.greet("Ada".to_owned());
    let _: restate_sdk::ingress::Request<MacroTestExecutor, (), ()> = service.no_input();

    let object = MyObjIngressClient::from_client(client.clone(), "object-key");
    let _: restate_sdk::ingress::Request<MacroTestExecutor, u64, u64> = object.add(1);
    let _: restate_sdk::ingress::Request<MacroTestExecutor, (), u64> = object.get();

    let workflow = MyWfIngressClient::from_client(client.clone(), String::from("workflow-key"));
    let _: restate_sdk::ingress::Request<MacroTestExecutor, String, String> =
        workflow.run("input".to_owned());
    let _: restate_sdk::ingress::Request<MacroTestExecutor, (), ()> = workflow.signal();

    // Covers service lifetimes, type generics, a where-clause, configured visibility/name, the
    // collision-free executor identifier, and a handler actually named `new`.
    let generic =
        GenericIngressNamesIngressClient::<'static, English, MacroTestExecutor>::from_client(
            client.clone(),
        );
    let _: restate_sdk::ingress::Request<MacroTestExecutor, String, String> =
        generic.new("hello".to_owned());

    // `client_visibility` applies to both generated client kinds.
    let restricted = RestrictedIngressClient::from_client(client);
    let _: restate_sdk::ingress::Request<MacroTestExecutor, (), ()> = restricted.ping();
}
