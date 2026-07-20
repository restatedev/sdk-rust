use restate_sdk::prelude::*;

struct WfRetentionOnService;

// `workflow_completion_retention` is only valid on `#[workflow]`, not `#[service]`/`#[object]`.
#[service(workflow_completion_retention = "1 day")]
impl WfRetentionOnService {
    #[handler]
    async fn greet(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

fn main() {}
