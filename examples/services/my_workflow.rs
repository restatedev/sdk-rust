use restate_sdk::prelude::*;

pub struct MyWorkflow;

#[restate_sdk::workflow(vis = "pub(crate)")]
impl MyWorkflow {
    #[handler]
    async fn run(&self, _ctx: WorkflowContext<'_>, _req: String) -> Result<String, HandlerError> {
        // implement workflow logic here

        Ok(String::from("success"))
    }

    #[handler(shared)]
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
    HttpServer::new(Endpoint::builder().bind(MyWorkflow.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
