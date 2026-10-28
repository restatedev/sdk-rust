#[path = "support/context_benchmark.rs"]
#[allow(dead_code)]
mod benchmark;

use benchmark::{Operation, measure};
use restate_sdk::prelude::*;

struct StartedRuns(usize);
#[service]
impl StartedRuns {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<u64> {
        let runs = (0..self.0)
            .map(|i| ctx.run(move || async move { Ok(i as u64) }).start())
            .collect::<Vec<_>>();
        Ok(futures::future::try_join_all(runs).await?.iter().sum())
    }
}

// cargo test --release --test started_run_benchmark -- --ignored --nocapture
#[tokio::test(flavor = "current_thread")]
#[ignore = "prints release timings; run without other compiler or test load"]
async fn started_run_timings() {
    for operations in [1, 10, 100, 1000] {
        measure(
            "started_run",
            Endpoint::builder().bind(StartedRuns(operations)).build(),
            "StartedRuns",
            Operation::Run,
            operations,
            (operations * (operations - 1) / 2) as u64,
        )
        .await;
    }
}
