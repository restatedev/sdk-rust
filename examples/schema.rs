//! Run with auto-generated schemas for `Json<Product>` using `schemars`:
//!   cargo run --example schema --features schemars
//!
//! Run with primitive schemas only:
//!   cargo run --example schema

use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
struct Product {
    id: String,
    name: String,
    price_cents: u32,
}

#[restate_sdk::handler]
async fn get_product_by_id(
    ctx: Context<'_>,
    product_id: String,
) -> Result<Json<Product>, HandlerError> {
    ctx.sleep(Duration::from_millis(50)).await?;
    Ok(Json(Product {
        id: product_id,
        name: "Sample Product".to_string(),
        price_cents: 1995,
    }))
}

#[restate_sdk::handler]
async fn save_product(_ctx: Context<'_>, product: Json<Product>) -> Result<String, HandlerError> {
    Ok(product.0.id)
}

#[restate_sdk::handler]
async fn is_in_stock(_ctx: Context<'_>, product_id: String) -> Result<bool, HandlerError> {
    Ok(!product_id.contains("out-of-stock"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let catalog = service!(
        "CatalogService",
        get_product_by_id,
        save_product,
        is_in_stock
    );
    HttpServer::new(Endpoint::builder().bind(catalog).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
