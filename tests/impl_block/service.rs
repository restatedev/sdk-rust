use restate_sdk::prelude::*;

// Should compile

pub(crate) struct MyService;

#[allow(dead_code)]
#[restate_sdk::service(vis = "pub(crate)")]
impl MyService {
    #[handler]
    async fn my_handler(&self, _ctx: Context<'_>, _input: String) -> HandlerResult<String> {
        unimplemented!()
    }

    #[handler]
    async fn no_input(&self, _ctx: Context<'_>) -> HandlerResult<String> {
        unimplemented!()
    }

    #[handler]
    async fn no_output(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        unimplemented!()
    }

    #[handler]
    async fn no_input_no_output(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        unimplemented!()
    }

    #[handler]
    async fn std_result(&self, _ctx: Context<'_>) -> Result<(), std::io::Error> {
        unimplemented!()
    }

    #[handler]
    async fn std_result_with_terminal_error(&self, _ctx: Context<'_>) -> Result<(), TerminalError> {
        unimplemented!()
    }

    #[handler]
    async fn std_result_with_handler_error(&self, _ctx: Context<'_>) -> Result<(), HandlerError> {
        unimplemented!()
    }
}

pub(crate) struct MyObject;

#[allow(dead_code)]
#[restate_sdk::object(vis = "pub(crate)")]
impl MyObject {
    #[handler]
    async fn my_handler(&self, _ctx: ObjectContext<'_>, _input: String) -> HandlerResult<String> {
        unimplemented!()
    }

    #[handler(shared)]
    async fn my_shared_handler(
        &self,
        _ctx: SharedObjectContext<'_>,
        _input: String,
    ) -> HandlerResult<String> {
        unimplemented!()
    }
}

pub(crate) struct MyWorkflow;

#[allow(dead_code)]
#[restate_sdk::workflow(vis = "pub(crate)")]
impl MyWorkflow {
    #[handler]
    async fn my_handler(&self, _ctx: WorkflowContext<'_>, _input: String) -> HandlerResult<String> {
        unimplemented!()
    }

    #[handler(shared)]
    async fn my_shared_handler(
        &self,
        _ctx: SharedWorkflowContext<'_>,
        _input: String,
    ) -> HandlerResult<String> {
        unimplemented!()
    }
}

pub(crate) struct MyRenamedService;

#[restate_sdk::service(vis = "pub(crate)", name = "myRenamedService")]
impl MyRenamedService {
    #[handler(name = "myRenamedHandler")]
    async fn my_handler(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

#[test]
fn renamed_service_handler() {
    use restate_sdk::service::Discoverable;

    let discovery = ServeMyRenamedService::<MyRenamedService>::discover();
    assert_eq!(discovery.name.to_string(), "myRenamedService");
    assert_eq!(discovery.handlers[0].name.to_string(), "myRenamedHandler");
}
