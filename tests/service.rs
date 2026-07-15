use restate_sdk::prelude::*;

// Should compile
#[restate_sdk::service]
trait MyService {
    async fn my_handler(input: String) -> HandlerResult<String>;

    async fn no_input() -> HandlerResult<String>;

    async fn no_output() -> HandlerResult<()>;

    async fn no_input_no_output() -> HandlerResult<()>;

    async fn std_result() -> Result<(), std::io::Error>;

    async fn std_result_with_terminal_error() -> Result<(), TerminalError>;

    async fn std_result_with_handler_error() -> Result<(), HandlerError>;
}

#[restate_sdk::object]
trait MyObject {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

#[restate_sdk::workflow]
trait MyWorkflow {
    async fn my_handler(input: String) -> HandlerResult<String>;
    #[shared]
    async fn my_shared_handler(input: String) -> HandlerResult<String>;
}

#[restate_sdk::service]
#[name = "myRenamedService"]
trait MyRenamedService {
    #[name = "myRenamedHandler"]
    async fn my_handler() -> HandlerResult<()>;
}

struct MyRenamedServiceImpl;

impl MyRenamedService for MyRenamedServiceImpl {
    async fn my_handler(&self, _: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn renamed_service_handler() {
    use restate_sdk::service::Discoverable;

    let discovery = ServeMyRenamedService::<MyRenamedServiceImpl>::discover();
    assert_eq!(discovery.name.to_string(), "myRenamedService");
    assert_eq!(discovery.handlers[0].name.to_string(), "myRenamedHandler");
}

// ============================ New function-first API ============================

// Service handlers, covering all the input/output/error shapes.
#[restate_sdk::handler]
async fn fn_my_handler(ctx: Context<'_>, input: String) -> HandlerResult<String> {
    let _ = ctx.extension::<u32>(); // ambient DI compiles on all context types
    Ok(input)
}
#[restate_sdk::handler]
async fn fn_no_input(_ctx: Context<'_>) -> HandlerResult<String> {
    Ok("x".to_owned())
}
#[restate_sdk::handler]
async fn fn_no_output(_ctx: Context<'_>, _input: String) -> HandlerResult<()> {
    Ok(())
}
#[restate_sdk::handler]
async fn fn_no_input_no_output(_ctx: Context<'_>) -> HandlerResult<()> {
    Ok(())
}
#[restate_sdk::handler]
async fn fn_std_result(_ctx: Context<'_>) -> Result<(), std::io::Error> {
    Ok(())
}
#[restate_sdk::handler]
async fn fn_terminal(_ctx: Context<'_>) -> Result<(), TerminalError> {
    Ok(())
}
#[restate_sdk::handler(name = "myRenamedFnHandler")]
async fn fn_renamed(_ctx: Context<'_>) -> HandlerResult<()> {
    Ok(())
}

// Object handlers: exclusive + shared (shared inferred from the context type).
#[restate_sdk::handler]
async fn obj_exclusive(_ctx: ObjectContext<'_>, input: String) -> HandlerResult<String> {
    Ok(input)
}
#[restate_sdk::handler]
async fn obj_shared(_ctx: SharedObjectContext<'_>, input: String) -> HandlerResult<String> {
    Ok(input)
}

// Workflow handlers: run + shared.
#[restate_sdk::handler]
async fn wf_run(_ctx: WorkflowContext<'_>, input: String) -> HandlerResult<String> {
    Ok(input)
}
#[restate_sdk::handler]
async fn wf_shared(_ctx: SharedWorkflowContext<'_>, input: String) -> HandlerResult<String> {
    Ok(input)
}

// Declarative item macros define the service types (+ clients). Distinct names from the
// deprecated trait-based `MyObject`/`MyWorkflow` above.
service!(FnService: {
    fn_my_handler,
    fn_no_input,
    fn_no_output,
    fn_no_input_no_output,
    fn_std_result,
    fn_terminal,
    fn_renamed,
});
object!(FnObject: { obj_exclusive, obj_shared });
workflow!(FnWorkflow: { wf_run, wf_shared });

#[test]
fn fn_handlers_meta_and_composition() {
    use restate_sdk::service::Handler;

    // meta() is generated correctly, including name override.
    assert_eq!(fn_my_handler.meta().name.as_ref(), "fn_my_handler");
    assert_eq!(fn_renamed.meta().name.as_ref(), "myRenamedFnHandler");

    // The body remains directly callable for unit tests.
    // (compile-only: we don't have a Context here)
    let _call = fn_no_input_no_output::call;

    // The declarative macros define bindable service types; `.with_extension(..)` attaches a
    // service-scoped dependency.
    let _def = FnService.with_extension(0u32);
    let _kinds = (FnObject, FnWorkflow);
}

// The generated client exposes typed methods (compile-only; a real client needs a live context).
#[allow(dead_code)]
fn client_compiles<'ctx>(client: &FnServiceClient<'ctx>) -> Request<'ctx, String, String> {
    client.fn_my_handler("hi".to_string())
}
