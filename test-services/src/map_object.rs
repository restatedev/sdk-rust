use anyhow::anyhow;
use restate_sdk::prelude::*;
use restate_sdk::service::ServiceDefinition;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    key: String,
    value: String,
}

#[restate_sdk::handler]
pub(crate) async fn set(
    ctx: ObjectContext<'_>,
    Json(Entry { key, value }): Json<Entry>,
) -> HandlerResult<()> {
    ctx.set(&key, value);
    Ok(())
}

#[restate_sdk::handler]
pub(crate) async fn get(ctx: ObjectContext<'_>, key: String) -> HandlerResult<String> {
    Ok(ctx.get(&key).await?.unwrap_or_default())
}

#[restate_sdk::handler(name = "clearAll")]
pub(crate) async fn clear_all(ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<Entry>>> {
    let keys = ctx.get_keys().await?;

    let mut entries = vec![];
    for k in keys {
        let value = ctx
            .get(&k)
            .await?
            .ok_or_else(|| anyhow!("Missing key {k}"))?;
        entries.push(Entry { key: k, value })
    }

    ctx.clear_all();

    Ok(entries.into())
}

pub(crate) fn definition() -> ServiceDefinition {
    define_object("MapObject")
        .handler(set)
        .handler(get)
        .handler(clear_all)
        .build()
}
