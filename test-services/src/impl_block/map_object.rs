use anyhow::anyhow;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    key: String,
    value: String,
}

pub(crate) struct MapObject;

#[restate_sdk::object(vis = "pub(crate)", name = "MapObject")]
impl MapObject {
    #[handler(name = "set")]
    async fn set(
        &self,
        ctx: ObjectContext<'_>,
        Json(Entry { key, value }): Json<Entry>,
    ) -> HandlerResult<()> {
        ctx.set(&key, value);
        Ok(())
    }

    #[handler(name = "get")]
    async fn get(&self, ctx: ObjectContext<'_>, key: String) -> HandlerResult<String> {
        Ok(ctx.get(&key).await?.unwrap_or_default())
    }

    #[handler(name = "clearAll")]
    async fn clear_all(&self, ctx: ObjectContext<'_>) -> HandlerResult<Json<Vec<Entry>>> {
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
}
