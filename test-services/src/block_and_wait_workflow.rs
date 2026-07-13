use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;

const MY_PROMISE: &str = "my-promise";
const MY_STATE: &str = "my-state";

#[restate_sdk::handler]
pub(crate) async fn run(context: WorkflowContext<'_>, input: String) -> HandlerResult<String> {
    context.set(MY_STATE, input);

    let promise: String = context.promise(MY_PROMISE).await?;

    if context.peek_promise::<String>(MY_PROMISE).await?.is_none() {
        return Err(TerminalError::new("Durable promise should be completed").into());
    }

    Ok(promise)
}

#[restate_sdk::handler]
pub(crate) async fn unblock(
    context: SharedWorkflowContext<'_>,
    output: String,
) -> HandlerResult<()> {
    context.resolve_promise(MY_PROMISE, output);
    Ok(())
}

#[restate_sdk::handler(name = "getState")]
pub(crate) async fn get_state(
    context: SharedWorkflowContext<'_>,
) -> HandlerResult<Json<Option<String>>> {
    Ok(Json(context.get::<String>(MY_STATE).await?))
}

pub(crate) fn definition() -> ServiceDefinition {
    workflow!("BlockAndWaitWorkflow", run, unblock, get_state)
}
