use restate_sdk::prelude::*;

#[restate_sdk::handler]
pub async fn run(_ctx: WorkflowContext<'_>, _req: String) -> Result<String, HandlerError> {
    // implement workflow logic here
    Ok(String::from("success"))
}

#[restate_sdk::handler]
pub async fn interact_with_workflow(_ctx: SharedWorkflowContext<'_>) -> Result<(), HandlerError> {
    // implement interaction logic here
    // e.g. resolve a promise that the workflow is waiting on
    Ok(())
}

// Defines MyWorkflow
workflow!(MyWorkflow: { run, interact_with_workflow });
