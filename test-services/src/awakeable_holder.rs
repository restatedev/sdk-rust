use restate_sdk::prelude::*;

#[restate_sdk::object]
#[name = "AwakeableHolder"]
pub(crate) trait AwakeableHolder {
    #[name = "hold"]
    async fn hold(id: String) -> HandlerResult<()>;
    #[name = "hasAwakeable"]
    #[shared]
    async fn has_awakeable() -> HandlerResult<bool>;
    #[name = "unlock"]
    async fn unlock(payload: String) -> HandlerResult<()>;
}

pub(crate) struct AwakeableHolderImpl;

const ID: &str = "id";

impl AwakeableHolder for AwakeableHolderImpl {
    async fn hold(&self, context: ObjectContext<'_>, id: String) -> HandlerResult<()> {
        context.set(ID, id);
        Ok(())
    }

    async fn has_awakeable(&self, context: SharedObjectContext<'_>) -> HandlerResult<bool> {
        Ok(context.get::<String>(ID).await?.is_some())
    }

    async fn unlock(&self, context: ObjectContext<'_>, payload: String) -> HandlerResult<()> {
        let k: String = context.get(ID).await?.ok_or_else(|| {
            TerminalError::new(format!(
                "No awakeable stored for awakeable holder {}",
                context.key()
            ))
        })?;
        context.resolve_awakeable(&k, payload);
        Ok(())
    }
}
