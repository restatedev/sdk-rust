use restate_sdk::prelude::*;
use std::time::Duration;

// This example shows how to implement a periodic task, by invoking itself in a loop.
//
// The `start()` handler schedules the first call to `run()`, and then each `run()` will re-schedule itself.
//
// To "break" the loop, we use a flag we persist in state, which is removed when `stop()` is invoked.
// Its presence determines whether the task is active or not.
//
// To start it:
//
//   $ curl -v http://localhost:8080/PeriodicTask/my-periodic-task/start
//
// The interface generates `PeriodicTaskClient` (used for the self-call below) and the
// conformance-checked `PeriodicTask::server` builder.
restate_sdk::interface! {
    object PeriodicTask {
        start() -> ();
        stop() -> ();
        run() -> ();
    }
}

const ACTIVE: &str = "active";

fn schedule_next(ctx: &ObjectContext<'_>) {
    // To schedule, create a client to the callee handler (in this case, we're calling ourselves)
    ctx.object_client::<PeriodicTaskClient>(ctx.key())
        .run()
        // And send with a delay
        .send_after(Duration::from_secs(10));
}

/// Schedules the periodic task to start
#[restate_sdk::handler]
async fn start(ctx: ObjectContext<'_>) -> Result<(), TerminalError> {
    if ctx.get::<bool>(ACTIVE).await?.is_some_and(|enabled| enabled) {
        // If it's already activated, just do nothing
        return Ok(());
    }

    // Schedule the periodic task
    schedule_next(&ctx);

    // Mark the periodic task as active
    ctx.set(ACTIVE, true);

    Ok(())
}

/// Stops the periodic task
#[restate_sdk::handler]
async fn stop(ctx: ObjectContext<'_>) -> Result<(), TerminalError> {
    // Remove the active flag
    ctx.clear(ACTIVE);

    Ok(())
}

/// Business logic of the periodic task
#[restate_sdk::handler]
async fn run(ctx: ObjectContext<'_>) -> Result<(), TerminalError> {
    if ctx.get::<bool>(ACTIVE).await?.is_none() {
        // Task is inactive, do nothing
        return Ok(());
    }

    // --- Periodic task business logic!
    println!("Triggered the periodic task!");

    // Schedule the periodic task
    schedule_next(&ctx);

    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let task = PeriodicTask::from_handlers(PeriodicTaskHandlers { start, stop, run });
    HttpServer::new(Endpoint::builder().bind(task).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
