use restate_sdk::prelude::*;

pub(crate) struct AwakeableHolder;

const ID: &str = "id";

#[restate_sdk::object(vis = "pub(crate)", name = "AwakeableHolder")]
impl AwakeableHolder {
    #[handler(name = "hold")]
    async fn hold(&self, context: ObjectContext<'_>, id: String) -> HandlerResult<()> {
        context.set(ID, id);
        Ok(())
    }

    #[handler(shared, name = "hold")]
    async fn has_awakeable(&self, context: SharedObjectContext<'_>) -> HandlerResult<bool> {
        Ok(context.get::<String>(ID).await?.is_some())
    }

    #[handler(name = "unlock")]
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
