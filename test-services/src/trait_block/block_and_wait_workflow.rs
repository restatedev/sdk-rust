use restate_sdk::prelude::*;

#[restate_sdk::workflow]
#[name = "BlockAndWaitWorkflow"]
pub(crate) trait BlockAndWaitWorkflow {
    #[name = "run"]
    async fn run(input: String) -> HandlerResult<String>;
    #[name = "unblock"]
    #[shared]
    async fn unblock(output: String) -> HandlerResult<()>;
    #[name = "getState"]
    #[shared]
    async fn get_state() -> HandlerResult<Json<Option<String>>>;
}

pub(crate) struct BlockAndWaitWorkflowImpl;

const MY_PROMISE: &str = "my-promise";
const MY_STATE: &str = "my-state";

impl BlockAndWaitWorkflow for BlockAndWaitWorkflowImpl {
    async fn run(&self, context: WorkflowContext<'_>, input: String) -> HandlerResult<String> {
        context.set(MY_STATE, input);

        let promise: String = context.promise(MY_PROMISE).await?;

        if context.peek_promise::<String>(MY_PROMISE).await?.is_none() {
            return Err(TerminalError::new("Durable promise should be completed").into());
        }

        Ok(promise)
    }

    async fn unblock(
        &self,
        context: SharedWorkflowContext<'_>,
        output: String,
    ) -> HandlerResult<()> {
        context.resolve_promise(MY_PROMISE, output);
        Ok(())
    }

    async fn get_state(
        &self,
        context: SharedWorkflowContext<'_>,
    ) -> HandlerResult<Json<Option<String>>> {
        Ok(Json(context.get::<String>(MY_STATE).await?))
    }
}
