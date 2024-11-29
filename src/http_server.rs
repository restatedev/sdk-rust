//! # Serving
//! Restate services run as an HTTP endpoint.
//!
//! ## Creating an HTTP endpoint
//! 1. Create the endpoint
//! 2. Bind one or multiple services to it.
//! 3. Listen on the specified port (default `9080`) for connections and requests.
//!
//! ```
//! # #[path = "../examples/services"]
//! # mod services;
//! # use services::my_service::{MyService, MyServiceImpl};
//! # use services::my_virtual_object::{MyVirtualObject, MyVirtualObjectImpl};
//! # use services::my_workflow::{MyWorkflow, MyWorkflowImpl};
//! use restate_sdk::endpoint::Endpoint;
//! use restate_sdk::http_server::HttpServer;
//!
//! #[tokio::main]
//! async fn main() {
//!     tracing_subscriber::fmt::init();
//!     HttpServer::new(
//!         Endpoint::builder()
//!             .bind(MyServiceImpl.serve())
//!             .bind(MyVirtualObjectImpl.serve())
//!             .bind(MyWorkflowImpl.serve())
//!             .build(),
//!     )
//!     .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!     .await;
//! }
//! ```
//!
//!
//! ## Validating request identity
//!
//! SDKs can validate that incoming requests come from a particular Restate
//! instance. You can find out more about request identity in the [Security docs](https://docs.restate.dev/operate/security#locking-down-service-access).
//! Add the identity key to your endpoint as follows:
//!
//! ```
//! # #[path = "../examples/services"]
//! # mod services;
//! # use services::my_service::{MyService, MyServiceImpl};
//! # use restate_sdk::endpoint::Endpoint;
//! # use restate_sdk::http_server::HttpServer;
//! #
//! # #[tokio::main]
//! # async fn main() {
//!  #    tracing_subscriber::fmt::init();
//!     HttpServer::new(
//!         Endpoint::builder()
//!             .bind(MyServiceImpl.serve())
//!             .identity_key("publickeyv1_w7YHemBctH5Ck2nQRQ47iBBqhNHy4FV7t2Usbye2A6f")
//!             .unwrap()
//!             .build(),
//!     )
//!     .listen_and_serve("0.0.0.0:9080".parse().unwrap())
//!     .await;
//! # }
//! ```

use crate::endpoint::Endpoint;
use crate::hyper::HyperEndpoint;
use futures::FutureExt;
use hyper::server::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// Http server to expose your Restate services.
pub struct HttpServer {
    endpoint: Endpoint,
}

impl From<Endpoint> for HttpServer {
    fn from(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl HttpServer {
    /// Create new [`HttpServer`] from an [`Endpoint`].
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// Listen on the given address and serve.
    ///
    /// The future will be completed once `SIGTERM` is sent to the process.
    pub async fn listen_and_serve(self, addr: SocketAddr) {
        let listener = TcpListener::bind(addr).await.expect("listener can bind");
        self.serve(listener).await;
    }

    /// Serve on the given listener.
    ///
    /// The future will be completed once `SIGTERM` is sent to the process.
    pub async fn serve(self, listener: TcpListener) {
        self.serve_with_cancel(listener, tokio::signal::ctrl_c().map(|_| ()))
            .await;
    }

    /// Serve on the given listener, and cancel the execution with the given future.
    pub async fn serve_with_cancel(self, listener: TcpListener, cancel_signal_future: impl Future) {
        let endpoint = HyperEndpoint::new(self.endpoint);
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();

        // when this signal completes, start shutdown
        let mut signal = std::pin::pin!(cancel_signal_future);

        info!("Starting listening on {}", listener.local_addr().unwrap());

        // Our server accept loop
        loop {
            tokio::select! {
                Ok((stream, remote)) = listener.accept() => {
                    let endpoint = endpoint.clone();

                    let conn = http2::Builder::new(TokioExecutor::default())
                        .serve_connection(TokioIo::new(stream), endpoint);

                    let fut = graceful.watch(conn);

                    tokio::spawn(async move {
                        if let Err(e) = fut.await {
                            warn!("Error serving connection {remote}: {:?}", e);
                        }
                    });
                },
                _ = &mut signal => {
                    info!("Shutting down");
                    // stop the accept loop
                    break;
                }
            }
        }

        // Wait graceful shutdown
        tokio::select! {
            _ = graceful.shutdown() => {},
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                warn!("Timed out waiting for all connections to close");
            }
        }
    }
}
