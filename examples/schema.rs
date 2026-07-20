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
pub struct Product {
    id: String,
    name: String,
    price_cents: u32,
}

struct CatalogService;

#[service]
impl CatalogService {
    #[handler]
    async fn get_product_by_id(
        &self,
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

    #[handler]
    async fn save_product(
        &self,
        _ctx: Context<'_>,
        product: Json<Product>,
    ) -> Result<String, HandlerError> {
        Ok(product.0.id)
    }

    #[handler]
    async fn is_in_stock(
        &self,
        _ctx: Context<'_>,
        product_id: String,
    ) -> Result<bool, HandlerError> {
        Ok(!product_id.contains("out-of-stock"))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    HttpServer::new(Endpoint::builder().bind(CatalogService).build())
        .listen_and_serve("0.0.0.0:9080".parse().unwrap())
        .await;
}
