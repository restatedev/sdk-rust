use restate_sdk::prelude::*;

const ID: &str = "id";

#[restate_sdk::handler]
pub(crate) async fn hold(context: ObjectContext<'_>, id: String) -> HandlerResult<()> {
    context.set(ID, id);
    Ok(())
}

#[restate_sdk::handler(name = "hasAwakeable")]
pub(crate) async fn has_awakeable(context: SharedObjectContext<'_>) -> HandlerResult<bool> {
    Ok(context.get::<String>(ID).await?.is_some())
}

#[restate_sdk::handler]
pub(crate) async fn unlock(context: ObjectContext<'_>, payload: String) -> HandlerResult<()> {
    let k: String = context.get(ID).await?.ok_or_else(|| {
        TerminalError::new(format!(
            "No awakeable stored for awakeable holder {}",
            context.key()
        ))
    })?;
    context.resolve_awakeable(&k, payload);
    Ok(())
}

// Defines `AwakeableHolder` + `AwakeableHolderClient` (used by the cancel/kill test services).
object!(AwakeableHolder: { hold, has_awakeable, unlock });
