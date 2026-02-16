use restate_sdk::prelude::*;

/// This example demonstrates the new #[restate(...)] attribute syntax
/// for configuring handler behavior.
#[restate_sdk::object]
trait ProductInventory {
    /// Shared handler with timeout configuration
    /// This handler can run concurrently with others and has a 30-second inactivity timeout
    #[restate(shared, inactivity_timeout = "30s")]
    async fn get_stock() -> Result<u32, TerminalError>;

    /// Regular handler with custom timeouts
    /// This handler has exclusive access to state and custom timeout settings
    #[restate(inactivity_timeout = "1m", abort_timeout = "10s")]
    async fn update_stock(quantity: u32) -> Result<(), TerminalError>;

    /// Shared handler with lazy state enabled
    /// Useful for read-heavy operations where state is loaded on-demand
    #[restate(shared = true, lazy_state = true, inactivity_timeout = "45s")]
    async fn check_availability() -> Result<bool, TerminalError>;

    /// Regular handler without special configuration
    async fn reserve_stock(quantity: u32) -> Result<bool, TerminalError>;
    
    /// Handler demonstrating different duration formats
    #[restate(inactivity_timeout = "500ms", abort_timeout = "2h")]
    async fn quick_check() -> Result<(), TerminalError>;
}

struct ProductInventoryImpl;

const STOCK_KEY: &str = "stock";

impl ProductInventory for ProductInventoryImpl {
    async fn get_stock(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<u32, TerminalError> {
        println!("Getting stock");
        Ok(ctx.get::<u32>(STOCK_KEY).await?.unwrap_or(0))
    }

    async fn update_stock(
        &self,
        ctx: ObjectContext<'_>,
        quantity: u32,
    ) -> Result<(), TerminalError> {
        println!("Updating stock to {}", quantity);
        ctx.set(STOCK_KEY, quantity);
        Ok(())
    }

    async fn check_availability(
        &self,
        ctx: SharedObjectContext<'_>,
    ) -> Result<bool, TerminalError> {
        println!("Checking availability");
        let stock = ctx.get::<u32>(STOCK_KEY).await?.unwrap_or(0);
        Ok(stock > 0)
    }

    async fn reserve_stock(
        &self,
        ctx: ObjectContext<'_>,
        quantity: u32,
    ) -> Result<bool, TerminalError> {
        println!("Reserving {} units", quantity);
        let current = ctx.get::<u32>(STOCK_KEY).await?.unwrap_or(0);
        
        if current >= quantity {
            ctx.set(STOCK_KEY, current - quantity);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn quick_check(
        &self,
        _ctx: ObjectContext<'_>,
    ) -> Result<(), TerminalError> {
        println!("Quick check");
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    
    println!("Starting Product Inventory service with new #[restate(...)] attribute syntax");
    println!("This example demonstrates:");
    println!("  - Shared handlers with configurable timeouts");
    println!("  - Lazy state loading");
    println!("  - Various duration formats (ms, s, m, h)");
    println!("  - Mixing multiple configuration options");
    
    HttpServer::new(
        Endpoint::builder()
            .bind(ProductInventoryImpl.serve())
            .build()
    )
    .listen_and_serve("0.0.0.0:9080".parse().unwrap())
    .await;
}
