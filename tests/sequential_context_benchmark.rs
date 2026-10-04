#[path = "support/context_benchmark.rs"]
mod benchmark;

use benchmark::{Operation, measure};
use restate_sdk::prelude::*;
use std::time::Duration;

struct SequentialRuns;
#[service]
impl SequentialRuns {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<u64> {
        let mut sum = 0;
        for i in 0..1000u64 {
            sum += ctx.run(move || async move { Ok(i) }).await?;
        }
        Ok(sum)
    }
}

struct SequentialTimers;
#[service]
impl SequentialTimers {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<u64> {
        for _ in 0..1000 {
            ctx.sleep(Duration::from_secs(1)).await?;
        }
        Ok(1000)
    }
}

// cargo test --release --test sequential_context_benchmark -- --ignored --nocapture
#[tokio::test(flavor = "current_thread")]
#[ignore = "prints release timings; run without other compiler or test load"]
async fn sequential_context_timings() {
    measure(
        "sequential_run",
        Endpoint::builder().bind(SequentialRuns).build(),
        "SequentialRuns",
        Operation::Run,
        1000,
        499_500,
    )
    .await;
    measure(
        "sequential_timer",
        Endpoint::builder().bind(SequentialTimers).build(),
        "SequentialTimers",
        Operation::Timer,
        1000,
        1000,
    )
    .await;
}
