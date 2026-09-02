#![cfg(feature = "tunnel")]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use ed25519_dalek::SigningKey;
use http::{Method, Request, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo};
use restate_sdk::prelude::{Endpoint, Tunnel};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::task::AbortOnDropHandle;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const IDENTITY_SEED: [u8; 32] = [0x2a; 32];

type CloudBody = UnsyncBoxBody<Bytes, Infallible>;

/// One Cloud-side HTTP/2 session. The driver task is always aborted if a test
/// assertion unwinds before the explicit close checks have reaped it.
struct CloudSession {
    sender: SendRequest<CloudBody>,
    driver: AbortOnDropHandle<Result<(), hyper::Error>>,
    connection_id: String,
    trailers: Option<oneshot::Sender<http::HeaderMap>>,
}

impl CloudSession {
    async fn accept(listener: &TcpListener) -> Self {
        let (socket, _) = tokio::time::timeout(TEST_TIMEOUT, listener.accept())
            .await
            .expect("SDK did not open the replacement tunnel promptly")
            .expect("failed to accept tunnel socket");
        let (mut sender, connection) = hyper::client::conn::http2::handshake::<_, _, CloudBody>(
            TokioExecutor::new(),
            TokioIo::new(socket),
        )
        .await
        .expect("failed to start the Cloud side of HTTP/2");
        let driver = AbortOnDropHandle::new(tokio::spawn(connection));

        let (trailers, body) = handshake_body();
        let response = sender
            .send_request(
                Request::builder()
                    .method(Method::GET)
                    .uri("http://fake-tunnel.test/_/start-tunnel")
                    .body(body)
                    .unwrap(),
            )
            .await
            .expect("SDK did not answer /_/start-tunnel");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["supports-drain"], "true");
        let connection_id = response.headers()["tunnel-connection-id"]
            .to_str()
            .expect("connection id is not a header-safe string")
            .to_owned();
        uuid::Uuid::parse_str(&connection_id).expect("connection id must be a UUID");

        Self {
            sender,
            driver,
            connection_id,
            trailers: Some(trailers),
        }
    }

    fn establish(&mut self) {
        self.trailers
            .take()
            .expect("session established once")
            .send(ok_trailers())
            .expect("SDK dropped the handshake body before trailers");
    }

    async fn control(&mut self, path: &'static str) -> StatusCode {
        let response = self
            .sender
            .send_request(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("http://fake-tunnel.test{path}"))
                    .body(Empty::<Bytes>::new().boxed_unsync())
                    .unwrap(),
            )
            .await
            .expect("control request failed on a serving tunnel session");
        let status = response.status();
        response
            .into_body()
            .collect()
            .await
            .expect("control response body failed");
        status
    }

    async fn wait_closed(&mut self) {
        let result = tokio::time::timeout(TEST_TIMEOUT, &mut self.driver)
            .await
            .expect("tunnel session task did not terminate after close")
            .expect("Cloud HTTP/2 driver task panicked");
        // An abrupt client close may be reported as either a clean EOF or an
        // H2 transport error. Completion, rather than its result, proves the
        // owned driver and socket are gone.
        let _ = result;
    }
}

fn handshake_body() -> (oneshot::Sender<http::HeaderMap>, CloudBody) {
    let (trailers_tx, trailers_rx) = oneshot::channel();
    let body = Empty::<Bytes>::new()
        .with_trailers(async move { trailers_rx.await.ok().map(Ok::<_, Infallible>) })
        .boxed_unsync();
    (trailers_tx, body)
}

fn ok_trailers() -> http::HeaderMap {
    let mut trailers = http::HeaderMap::new();
    trailers.insert("tunnel-status", "ok".parse().unwrap());
    trailers.insert("tunnel-name", "handover-test".parse().unwrap());
    trailers.insert(
        "proxy-url",
        "https://proxy.example/env_test123/handover-test"
            .parse()
            .unwrap(),
    );
    trailers.insert(
        "tunnel-url",
        "https://tunnel.example/handover-test".parse().unwrap(),
    );
    trailers
}

fn tunnel(address: SocketAddr) -> Tunnel {
    let signing_key = SigningKey::from_bytes(&IDENTITY_SEED);
    let public_key = format!(
        "publickeyv1_{}",
        bs58::encode(signing_key.verifying_key().to_bytes()).into_string()
    );
    Tunnel::new(Endpoint::builder().build())
        .tunnel_servers([format!("http://{address}")])
        .environment_id("env_test123")
        .auth_token("test-token")
        .signing_public_key(public_key)
        .tunnel_name("handover-test")
        .tunnel_worker_id("worker-test")
}

#[tokio::test]
async fn server_drain_overlaps_replacement_then_close_reaps_both_sessions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut connect = AbortOnDropHandle::new(tokio::spawn(tunnel(address).connect()));

    let mut first = CloudSession::accept(&listener).await;
    first.establish();
    let connection = tokio::time::timeout(TEST_TIMEOUT, &mut connect)
        .await
        .expect("connect did not observe the first successful handshake")
        .expect("connect task panicked")
        .expect("connect failed");

    assert_eq!(first.control("/_/drain-tunnel").await, StatusCode::OK);

    // The server drain is a handover request: a replacement must start right
    // away, while the established session continues serving. Keep the second
    // handshake's trailers pending while proving the overlap.
    let mut second = CloudSession::accept(&listener).await;
    assert_ne!(first.connection_id, second.connection_id);
    assert_eq!(first.control("/_/health").await, StatusCode::OK);
    second.establish();

    let close = connection.close();
    let ((), (), result) = tokio::join!(first.wait_closed(), second.wait_closed(), close);
    result.expect("closing the tunnel engine failed");
}
