use restate_sdk::prelude::*;
use std::time::Duration;

/// This example shows how to implement a periodic task, by invoking itself in a loop.
///
/// The `start()` handler schedules the first call to `run()`, and then each `run()` will re-schedule itself.
///
/// To "break" the loop, we use a flag we persist in state, which is removed when `stop()` is invoked.
/// Its presence determines whether the task is active or not.
///
/// To start it:
///
/// ```shell
/// $ curl -v http://localhost:8080/PeriodicTask/my-periodic-task/start
/// ```
struct PeriodicTask;

const ACTIVE: &str = "active";

#[restate_sdk::object]
impl PeriodicTask {
    #[handler]
    async fn start(&self, context: ObjectContext<'_>) -> Result<(), TerminalError> {
        if context
            .get::<bool>(ACTIVE)
            .await?
            .is_some_and(|enabled| enabled)
        {
            // If it's already activated, just do nothing
            return Ok(());
        }

        // Schedule the periodic task
        PeriodicTask::schedule_next(&context);

        // Mark the periodic task as active
        context.set(ACTIVE, true);

        Ok(())
    }

    #[handler]
    async fn stop(&self, context: ObjectContext<'_>) -> Result<(), TerminalError> {
        // Remove the active flag
        context.clear(ACTIVE);

        Ok(())
    }

    #[handler]
    async fn run(&self, context: ObjectContext<'_>) -> Result<(), TerminalError> {
        if context.get::<bool>(ACTIVE).await?.is_none() {
            // Task is inactive, do nothing
            return Ok(());
        }

        // --- Periodic task business logic!
        println!("Triggered the periodic task!");

        // Schedule the periodic task
        PeriodicTask::schedule_next(&context);

        Ok(())
    }
}

impl PeriodicTask {
    fn schedule_next(context: &ObjectContext<'_>) {
        // To schedule, create a client to the callee handler (in this case, we're calling ourselves)
        context
            .object_client::<PeriodicTaskClient>(context.key())
            .run()
            // And send with a delay
            .send_after(Duration::from_secs(10));
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(PeriodicTask.serve()).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
