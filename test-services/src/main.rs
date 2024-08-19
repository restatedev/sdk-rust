mod counter;
mod map_object;
mod proxy;

use restate_sdk::prelude::{Endpoint, HyperServer};
use std::env;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port = env::var("PORT").ok().unwrap_or("9080".to_string());
    let services = env::var("SERVICES").ok().unwrap_or("*".to_string());

    let mut builder = Endpoint::builder();

    if services == "*" || services.contains("Counter") {
        builder = builder.with_service(counter::Counter::serve(counter::CounterImpl))
    }
    if services == "*" || services.contains("Proxy") {
        builder = builder.with_service(proxy::Proxy::serve(proxy::ProxyImpl))
    }
    if services == "*" || services.contains("MapObject") {
        builder = builder.with_service(map_object::MapObject::serve(map_object::MapObjectImpl))
    }

    HyperServer::new(builder.build())
        .listen_and_serve(format!("0.0.0.0:{port}").parse().unwrap())
        .await;
}
