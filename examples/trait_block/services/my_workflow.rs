use restate_sdk::prelude::*;

#[restate_sdk::workflow]
pub trait MyWorkflow {
    async fn run(req: String) -> Result<String, HandlerError>;
    #[shared]
    async fn interact_with_workflow() -> Result<(), HandlerError>;
}

pub struct MyWorkflowImpl;

impl MyWorkflow for MyWorkflowImpl {
    async fn run(&self, _ctx: WorkflowContext<'_>, _req: String) -> Result<String, HandlerError> {
        // implement workflow logic here

        Ok(String::from("success"))
    }
    async fn interact_with_workflow(
        &self,
        _ctx: SharedWorkflowContext<'_>,
    ) -> Result<(), HandlerError> {
        // implement interaction logic here
        // e.g. resolve a promise that the workflow is waiting on

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(MyWorkflowImpl.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
