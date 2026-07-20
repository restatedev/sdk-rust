use restate_sdk::prelude::*;

struct WfRetentionOnHandler;

// `workflow_completion_retention` is configured on the `#[workflow]` attribute, not the handler.
#[workflow]
impl WfRetentionOnHandler {
    #[handler(workflow_completion_retention = "1 day")]
    async fn run(&self, _ctx: WorkflowContext<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
