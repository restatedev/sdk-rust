use crate::counter::CounterClient;
use restate_sdk::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[restate_sdk::object]
#[name = "NonDeterministic"]
pub(crate) trait NonDeterministic {
    #[name = "eitherSleepOrCall"]
    async fn either_sleep_or_call() -> HandlerResult<()>;
    #[name = "callDifferentMethod"]
    async fn call_different_method() -> HandlerResult<()>;
    #[name = "backgroundInvokeWithDifferentTargets"]
    async fn background_invoke_with_different_targets() -> HandlerResult<()>;
    #[name = "setDifferentKey"]
    async fn set_different_key() -> HandlerResult<()>;
}

#[derive(Clone, Default)]
pub(crate) struct NonDeterministicImpl(Arc<Mutex<HashMap<String, i32>>>);

const STATE_A: &str = "a";
const STATE_B: &str = "b";

impl NonDeterministic for NonDeterministicImpl {
    async fn either_sleep_or_call(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        if self.do_left_action(&context).await {
            context.sleep(Duration::from_millis(100)).await?;
        } else {
            context
                .object_client::<CounterClient>("abc")
                .get()
                .call()
                .await?;
        }
        Self::sleep_then_increment_counter(&context).await
    }

    async fn call_different_method(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        if self.do_left_action(&context).await {
            context
                .object_client::<CounterClient>("abc")
                .get()
                .call()
                .await?;
        } else {
            context
                .object_client::<CounterClient>("abc")
                .reset()
                .call()
                .await?;
        }
        Self::sleep_then_increment_counter(&context).await
    }

    async fn background_invoke_with_different_targets(
        &self,
        context: ObjectContext<'_>,
    ) -> HandlerResult<()> {
        if self.do_left_action(&context).await {
            context.object_client::<CounterClient>("abc").get().send();
        } else {
            context.object_client::<CounterClient>("abc").reset().send();
        }
        Self::sleep_then_increment_counter(&context).await
    }

    async fn set_different_key(&self, context: ObjectContext<'_>) -> HandlerResult<()> {
        if self.do_left_action(&context).await {
            context.set(STATE_A, "my-state".to_owned());
        } else {
            context.set(STATE_B, "my-state".to_owned());
        }
        Self::sleep_then_increment_counter(&context).await
    }
}

impl NonDeterministicImpl {
    async fn do_left_action(&self, ctx: &ObjectContext<'_>) -> bool {
        let mut counts = self.0.lock().await;
        *(counts
            .entry(ctx.key().to_owned())
            .and_modify(|i| *i += 1)
            .or_default())
            % 2
            == 1
    }

    async fn sleep_then_increment_counter(ctx: &ObjectContext<'_>) -> HandlerResult<()> {
        ctx.sleep(Duration::from_millis(100)).await?;
        ctx.object_client::<CounterClient>(ctx.key()).add(1).send();
        Ok(())
    }
}
