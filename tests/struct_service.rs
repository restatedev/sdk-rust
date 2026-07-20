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

#[test]
fn binds_without_serve() {
    // The struct value binds directly, no `.serve()`.
    let _ = Endpoint::builder()
        .bind(MySvc)
        .bind(MyObj)
        .bind(MyWf)
        .build();
}
