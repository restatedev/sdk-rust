use anyhow::anyhow;
use restate_sdk::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

#[restate_sdk::object]
#[name = "Failing"]
pub(crate) trait Failing {
    #[name = "terminallyFailingCall"]
    async fn terminally_failing_call(error_message: String) -> HandlerResult<()>;
    #[name = "callTerminallyFailingCall"]
    async fn call_terminally_failing_call(error_message: String) -> HandlerResult<String>;
    #[name = "failingCallWithEventualSuccess"]
    async fn failing_call_with_eventual_success() -> HandlerResult<i32>;
    #[name = "failingSideEffectWithEventualSuccess"]
    async fn failing_side_effect_with_eventual_success() -> HandlerResult<i32>;
    #[name = "terminallyFailingSideEffect"]
    async fn terminally_failing_side_effect(error_message: String) -> HandlerResult<()>;
}

#[derive(Clone, Default)]
pub(crate) struct FailingImpl {
    eventual_success_calls: Arc<AtomicI32>,
    eventual_success_side_effects: Arc<AtomicI32>,
}

impl Failing for FailingImpl {
    async fn terminally_failing_call(
        &self,
        _: ObjectContext<'_>,
        error_message: String,
    ) -> HandlerResult<()> {
        Err(TerminalError::new(error_message).into())
    }

    async fn call_terminally_failing_call(
        &self,
        mut context: ObjectContext<'_>,
        error_message: String,
    ) -> HandlerResult<String> {
        let uuid = context.rand_uuid().to_string();
        context
            .object_client::<FailingClient>(uuid)
            .terminally_failing_call(error_message)
            .call()
            .await?;

        unreachable!("This should be unreachable")
    }

    async fn failing_call_with_eventual_success(&self, _: ObjectContext<'_>) -> HandlerResult<i32> {
        let current_attempt = self.eventual_success_calls.fetch_add(1, Ordering::SeqCst) + 1;

        if current_attempt >= 4 {
            self.eventual_success_calls.store(0, Ordering::SeqCst);
            Ok(current_attempt)
        } else {
            Err(anyhow!("Failed at attempt ${current_attempt}").into())
        }
    }

    async fn failing_side_effect_with_eventual_success(
        &self,
        context: ObjectContext<'_>,
    ) -> HandlerResult<i32> {
        let cloned_eventual_side_effect_calls = Arc::clone(&self.eventual_success_side_effects);
        let success_attempt = context
            .run("failing_side_effect", || async move {
                let current_attempt =
                    cloned_eventual_side_effect_calls.fetch_add(1, Ordering::SeqCst) + 1;

                if current_attempt >= 4 {
                    cloned_eventual_side_effect_calls.store(0, Ordering::SeqCst);
                    Ok(current_attempt)
                } else {
                    Err(anyhow!("Failed at attempt ${current_attempt}").into())
                }
            })
            .await?;

        Ok(success_attempt)
    }

    async fn terminally_failing_side_effect(
        &self,
        context: ObjectContext<'_>,
        error_message: String,
    ) -> HandlerResult<()> {
        context
            .run("failing_side_effect", || async move {
                Err::<(), _>(TerminalError::new(error_message).into())
            })
            .await?;

        unreachable!("This should be unreachable")
    }
}
