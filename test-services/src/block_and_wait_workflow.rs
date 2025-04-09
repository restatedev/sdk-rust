use restate_sdk::prelude::*;

pub(crate) struct BlockAndWaitWorkflow;

const MY_PROMISE: &str = "my-promise";
const MY_STATE: &str = "my-state";

#[restate_sdk::workflow(vis = "pub(crate)", name = "BlockAndWaitWorkflow")]
impl BlockAndWaitWorkflow {
    #[handler(name = "run")]
    async fn run(&self, context: WorkflowContext<'_>, input: String) -> HandlerResult<String> {
        context.set(MY_STATE, input);

        let promise: String = context.promise(MY_PROMISE).await?;

        if context.peek_promise::<String>(MY_PROMISE).await?.is_none() {
            return Err(TerminalError::new("Durable promise should be completed").into());
        }

        Ok(promise)
    }

    #[handler(shared, name = "unblock")]
    async fn unblock(
        &self,
        context: SharedWorkflowContext<'_>,
        output: String,
    ) -> HandlerResult<()> {
        context.resolve_promise(MY_PROMISE, output);
        Ok(())
    }

    #[handler(shared, name = "getState")]
    async fn get_state(
        &self,
        context: SharedWorkflowContext<'_>,
    ) -> HandlerResult<Json<Option<String>>> {
        Ok(Json(context.get::<String>(MY_STATE).await?))
    }
}
