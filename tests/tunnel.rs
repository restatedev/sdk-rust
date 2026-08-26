#![cfg(feature = "tunnel")]

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use ed25519_dalek::{Signer, SigningKey};
use http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::{TokioExecutor, TokioIo};
use restate_sdk::prelude::{
    Context, Endpoint, HandlerResult, IntoServiceDefinition, ServiceOptions, Tunnel, service,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::task::AbortOnDropHandle;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const DISCOVERY_ACCEPT: &str = "application/vnd.restate.endpointmanifest.v2+json";
const IDENTITY_SEED: [u8; 32] = [0x2a; 32];
const LARGE_DISCOVERY_PADDING: usize = 4 * 1024 * 1024;

type CloudBody = UnsyncBoxBody<Bytes, Infallible>;

struct DrainProbe;

#[service]
impl DrainProbe {
    #[handler]
    async fn ping(&self, _ctx: Context<'_>) -> HandlerResult<()> {
        Ok(())
    }
}

/// A minimal role-reversed Cloud peer. It accepts the SDK's outbound socket,
/// runs HTTP/2 as the client, and keeps the `/_/start-tunnel` request body open
/// until the test supplies request trailers.
struct FakeCloud {
    address: SocketAddr,
    handshake: Option<oneshot::Receiver<(Handshake, CloudSender)>>,
    disconnected: Option<oneshot::Receiver<()>>,
    sender: Option<CloudSender>,
    task: Option<JoinHandle<()>>,
}

type CloudSender = hyper::client::conn::http2::SendRequest<CloudBody>;

struct Handshake {
    status: StatusCode,
    credentials: HeaderMap,
    trailers: oneshot::Sender<HeaderMap>,
}

struct CloudResponse {
    status: StatusCode,
    body: Bytes,
}

impl FakeCloud {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (handshake_tx, handshake) = oneshot::channel();
        let (disconnected_tx, disconnected) = oneshot::channel();

        let task = tokio::spawn(async move {
            let result = async {
                let (socket, _) = listener.accept().await?;
                let (mut sender, connection) =
                    hyper::client::conn::http2::handshake::<_, _, CloudBody>(
                        TokioExecutor::new(),
                        TokioIo::new(socket),
                    )
                    .await?;
                let mut connection = AbortOnDropHandle::new(tokio::spawn(connection));

                let (trailers, body) = handshake_body();
                let request = Request::builder()
                    .method(Method::GET)
                    .uri("http://fake-tunnel.test/_/start-tunnel")
                    .body(body)
                    .unwrap();
                let observation = {
                    let response = sender.send_request(request).await?;
                    Handshake {
                        status: response.status(),
                        credentials: response.headers().clone(),
                        trailers,
                    }
                };
                let _ = handshake_tx.send((observation, sender.clone()));

                // Keep the h2 driver and request sender alive so tests can
                // model Cloud forwarding more streams on this same session.
                let _ = (&mut connection).await;
                drop(sender);
                Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("fake tunnel peer stopped: {error}");
            }
            let _ = disconnected_tx.send(());
        });

        Self {
            address,
            handshake: Some(handshake),
            disconnected: Some(disconnected),
            sender: None,
            task: Some(task),
        }
    }

    async fn handshake(&mut self) -> Handshake {
        let (handshake, sender) = tokio::time::timeout(
            TEST_TIMEOUT,
            self.handshake.take().expect("handshake requested once"),
        )
        .await
        .expect("SDK did not answer /_/start-tunnel")
        .expect("fake peer stopped before the handshake response");
        self.sender = Some(sender);
        handshake
    }

    async fn wait_for_disconnect(&mut self) {
        tokio::time::timeout(
            TEST_TIMEOUT,
            self.disconnected.take().expect("disconnect requested once"),
        )
        .await
        .expect("tunnel socket was not closed")
        .expect("disconnect notifier was dropped");
        if let Some(task) = self.task.take() {
            task.await.unwrap();
        }
    }

    async fn get(&self, path: &str, headers: HeaderMap) -> CloudResponse {
        let mut sender = self.request_sender();
        let response = tokio::time::timeout(
            TEST_TIMEOUT,
            sender.send_request(cloud_request(path, headers)),
        )
        .await
        .expect("forwarded request did not complete")
        .expect("forwarded request failed");
        let status = response.status();
        let body = tokio::time::timeout(TEST_TIMEOUT, response.into_body().collect())
            .await
            .expect("forwarded request did not complete")
            .expect("forwarded response body failed")
            .to_bytes();
        CloudResponse { status, body }
    }

    fn request_sender(&self) -> CloudSender {
        self.sender
            .as_ref()
            .expect("request sender is available after handshake")
            .clone()
    }
}

impl Drop for FakeCloud {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn handshake_body() -> (oneshot::Sender<HeaderMap>, CloudBody) {
    let (trailers_tx, trailers_rx) = oneshot::channel();
    let body = Empty::<Bytes>::new()
        .with_trailers(async move { trailers_rx.await.ok().map(Ok::<_, Infallible>) })
        .boxed_unsync();
    (trailers_tx, body)
}

fn cloud_request(path: &str, headers: HeaderMap) -> Request<CloudBody> {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(format!("http://fake-tunnel.test{path}"))
        .body(Empty::<Bytes>::new().boxed_unsync())
        .unwrap();
    *request.headers_mut() = headers;
    request
}

fn tunnel(address: SocketAddr) -> Tunnel {
    configured_tunnel(address, Endpoint::builder().build())
}

fn configured_tunnel(address: SocketAddr, endpoint: Endpoint) -> Tunnel {
    Tunnel::new(endpoint)
        .tunnel_servers([format!("http://{address}")])
        .environment_id("env_test123")
        .auth_token("test-token")
        .signing_public_key(identity_public_key())
        .tunnel_name("greeter-test")
        .tunnel_worker_id("worker-test")
}

fn large_discovery_endpoint() -> Endpoint {
    let service = DrainProbe.into_service_definition().options(
        ServiceOptions::new().metadata("drain-test-padding", "x".repeat(LARGE_DISCOVERY_PADDING)),
    );
    Endpoint::builder().bind(service).build()
}

fn identity_public_key() -> String {
    let signing_key = SigningKey::from_bytes(&IDENTITY_SEED);
    format!(
        "publickeyv1_{}",
        bs58::encode(signing_key.verifying_key().to_bytes()).into_string()
    )
}

fn sign_identity(audience: &str) -> String {
    sign_identity_with_seed(audience, &IDENTITY_SEED)
}

fn sign_identity_with_seed(audience: &str, seed: &[u8; 32]) -> String {
    let signing_key = SigningKey::from_bytes(seed);
    let key_id = format!(
        "publickeyv1_{}",
        bs58::encode(signing_key.verifying_key().to_bytes()).into_string()
    );
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch")
        .as_secs();
    let header = serde_json::json!({
        "alg": "EdDSA",
        "typ": "JWT",
        "kid": key_id,
    });
    let claims = serde_json::json!({
        "aud": audience,
        "nbf": now.saturating_sub(60),
        "iat": now,
        "exp": now.saturating_add(60),
    });
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signed = format!("{header}.{claims}");
    let signature = signing_key.sign(signed.as_bytes());
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn identity_headers(audience: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("accept", DISCOVERY_ACCEPT.parse().unwrap());
    headers.insert("x-restate-signature-scheme", "v1".parse().unwrap());
    headers.insert("x-restate-jwt-v1", sign_identity(audience).parse().unwrap());
    headers
}

fn ok_trailers() -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert("tunnel-status", "ok".parse().unwrap());
    trailers.insert("tunnel-name", "greeter-test".parse().unwrap());
    trailers.insert(
        "proxy-url",
        "https://proxy.example/env_test123/greeter-test"
            .parse()
            .unwrap(),
    );
    trailers.insert(
        "tunnel-url",
        "https://tunnel.example/greeter-test".parse().unwrap(),
    );
    trailers
}

fn assert_credentials(handshake: &Handshake) {
    assert_eq!(handshake.status, StatusCode::OK);
    assert_eq!(handshake.credentials["authorization"], "Bearer test-token");
    assert_eq!(handshake.credentials["environment-id"], "env_test123");
    assert_eq!(handshake.credentials["tunnel-name"], "greeter-test");
    assert_eq!(handshake.credentials["tunnel-worker-id"], "worker-test");
    assert_eq!(handshake.credentials["supports-drain"], "true");
    assert_eq!(handshake.credentials["supports-client-drain"], "true");

    let connection_id = handshake.credentials["tunnel-connection-id"]
        .to_str()
        .unwrap();
    uuid::Uuid::parse_str(connection_id).expect("connection id must be a UUID");

    let credential_names = handshake
        .credentials
        .keys()
        .map(|name| name.as_str())
        // Hyper may supply generic representation metadata. Everything else
        // in these response headers belongs to the tunnel credentials.
        .filter(|name| !matches!(*name, "date" | "content-length"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        credential_names,
        BTreeSet::from([
            "authorization",
            "environment-id",
            "supports-client-drain",
            "supports-drain",
            "tunnel-connection-id",
            "tunnel-name",
            "tunnel-worker-id",
        ])
    );
}

async fn established_connection(cloud: &mut FakeCloud) -> restate_sdk::tunnel::TunnelConnection {
    established_connection_for(cloud, tunnel(cloud.address)).await
}

async fn established_connection_for(
    cloud: &mut FakeCloud,
    tunnel: Tunnel,
) -> restate_sdk::tunnel::TunnelConnection {
    let connect = AbortOnDropHandle::new(tokio::spawn(tunnel.connect()));
    let handshake = cloud.handshake().await;
    assert_credentials(&handshake);

    // Receiving response headers is not readiness. Cloud authorization is
    // carried in request trailers, so connect must remain pending here.
    tokio::task::yield_now().await;
    assert!(
        !connect.is_finished(),
        "connect returned before Cloud trailers"
    );

    handshake.trailers.send(ok_trailers()).unwrap();
    tokio::time::timeout(TEST_TIMEOUT, connect)
        .await
        .expect("connect did not observe successful trailers")
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn role_reversed_handshake_waits_for_trailers() {
    let mut cloud = FakeCloud::start().await;
    let connection = established_connection(&mut cloud).await;

    assert_eq!(connection.info().tunnel_name(), "greeter-test");
    assert_eq!(
        connection.info().proxy_url(),
        "https://proxy.example/env_test123/greeter-test"
    );
    assert_eq!(
        connection.info().tunnel_url(),
        "https://tunnel.example/greeter-test"
    );
    assert_eq!(
        connection.info().deployment_url(),
        "https://proxy.example:9080/env_test123/greeter-test/http/in-process/9080/"
    );

    let close = connection.close();
    cloud.wait_for_disconnect().await;
    close.await.unwrap();
}

#[tokio::test]
async fn cancelling_connect_while_trailers_are_pending_closes_the_session() {
    let mut cloud = FakeCloud::start().await;
    let connect = AbortOnDropHandle::new(tokio::spawn(tunnel(cloud.address).connect()));
    let handshake = cloud.handshake().await;
    assert_credentials(&handshake);

    // Keep Cloud's request body open: cancellation must tear down the engine,
    // its handshake reader, the H2 driver, and the socket without waiting for
    // trailers or for the five-second handshake deadline.
    drop(connect);
    cloud.wait_for_disconnect().await;
    drop(handshake);
}

#[tokio::test]
async fn forwarding_rewrites_before_identity_verification() {
    let mut cloud = FakeCloud::start().await;
    let endpoint = Endpoint::builder().bind(DrainProbe).build();
    let tunnel = configured_tunnel(cloud.address, endpoint);
    let connection = established_connection_for(&mut cloud, tunnel).await;

    let response = cloud
        .get("/http/h/9080/discover?x=1", identity_headers("/discover"))
        .await;
    assert_eq!(response.status, StatusCode::OK);
    let manifest: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(manifest["protocolMode"], "BIDI_STREAM");

    // A valid signed invocation reaches the real Endpoint route after rewrite.
    // Omitting the Restate protocol content type deliberately stops at the
    // Endpoint's media-type check without needing a synthetic protocol frame.
    let response = cloud
        .get(
            "/http/h/9080/invoke/DrainProbe/ping",
            identity_headers("/invoke/DrainProbe/ping"),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let mut missing_identity = HeaderMap::new();
    missing_identity.insert("accept", DISCOVERY_ACCEPT.parse().unwrap());
    let response = cloud.get("/http/h/9080/discover", missing_identity).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let response = cloud
        .get(
            "/http/h/9080/discover",
            identity_headers("/http/h/9080/discover"),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let mut wrong_key = identity_headers("/discover");
    wrong_key.insert(
        "x-restate-jwt-v1",
        sign_identity_with_seed("/discover", &[0x7c; 32])
            .parse()
            .unwrap(),
    );
    let response = cloud.get("/http/h/9080/discover", wrong_key).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    let response = cloud
        .get("/http/h/not-a-port/discover", identity_headers("/discover"))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    let close = connection.close();
    cloud.wait_for_disconnect().await;
    close.await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_closes_the_role_reversed_session() {
    let mut cloud = FakeCloud::start().await;
    let connection = established_connection(&mut cloud).await;

    // The lifecycle contract requires drain to begin when the consuming
    // method is called, not when its returned future is first polled.
    let shutdown = connection.shutdown_with_grace(Duration::from_secs(1));
    cloud.wait_for_disconnect().await;
    shutdown.await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_refuses_raced_requests_then_flushes_in_flight_response() {
    let mut cloud = FakeCloud::start().await;
    let tunnel = configured_tunnel(cloud.address, large_discovery_endpoint());
    let connection = established_connection_for(&mut cloud, tunnel).await;

    // Leave a response much larger than the HTTP/2 receive window unread.
    // The server has observed response-body EOS, but the peer has not yet
    // received all DATA, so graceful shutdown must keep the session alive.
    let mut held_sender = cloud.request_sender();
    let held_response = tokio::time::timeout(
        TEST_TIMEOUT,
        held_sender.send_request(cloud_request(
            "/http/h/9080/discover",
            identity_headers("/discover"),
        )),
    )
    .await
    .expect("held request did not receive response headers")
    .expect("held request failed");
    assert_eq!(held_response.status(), StatusCode::OK);

    // `send_request` queues the new stream synchronously. Calling shutdown
    // before polling its response models a stream racing the client's GOAWAY:
    // it must see the draining sentinel if it reaches the SDK.
    let mut raced_sender = cloud.request_sender();
    let raced_response = raced_sender.send_request(cloud_request(
        "/http/h/9080/discover",
        identity_headers("/discover"),
    ));
    let shutdown = connection.shutdown_with_grace(Duration::from_secs(2));

    let raced_response = tokio::time::timeout(TEST_TIMEOUT, raced_response)
        .await
        .expect("raced request did not complete")
        .expect("raced stream was reset instead of receiving the drain sentinel");
    assert_eq!(raced_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        raced_response.headers()["x-restate-tunnel-draining"],
        "true"
    );
    raced_response
        .into_body()
        .collect()
        .await
        .expect("draining response body failed");

    let mut shutdown = Box::pin(shutdown);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut shutdown)
            .await
            .is_err(),
        "shutdown completed before the unread response was flushed"
    );

    let held_body = tokio::time::timeout(TEST_TIMEOUT, held_response.into_body().collect())
        .await
        .expect("held response was not flushed during the grace period")
        .expect("held response body failed")
        .to_bytes();
    assert!(held_body.len() >= LARGE_DISCOVERY_PADDING);

    tokio::time::timeout(TEST_TIMEOUT, &mut shutdown)
        .await
        .expect("shutdown did not complete after the response flushed")
        .unwrap();
    cloud.wait_for_disconnect().await;
}

#[tokio::test]
async fn graceful_shutdown_force_closes_a_peer_that_does_not_read() {
    let mut cloud = FakeCloud::start().await;
    let tunnel = configured_tunnel(cloud.address, large_discovery_endpoint());
    let connection = established_connection_for(&mut cloud, tunnel).await;

    let mut sender = cloud.request_sender();
    let held_response = tokio::time::timeout(
        TEST_TIMEOUT,
        sender.send_request(cloud_request(
            "/http/h/9080/discover",
            identity_headers("/discover"),
        )),
    )
    .await
    .expect("held request did not receive response headers")
    .expect("held request failed");
    assert_eq!(held_response.status(), StatusCode::OK);

    tokio::time::timeout(
        TEST_TIMEOUT,
        connection.shutdown_with_grace(Duration::from_millis(30)),
    )
    .await
    .expect("grace deadline did not force-close the tunnel")
    .unwrap();
    cloud.wait_for_disconnect().await;

    // Keeping the body alive until after disconnect is the condition this
    // test exercises; dropping it now cannot be what allowed shutdown.
    drop(held_response);
}

#[tokio::test]
async fn dropping_connection_abruptly_closes_the_session() {
    let mut cloud = FakeCloud::start().await;
    let connection = established_connection(&mut cloud).await;

    drop(connection);
    cloud.wait_for_disconnect().await;
}

#[tokio::test]
async fn dropping_unpolled_shutdown_future_cannot_prevent_cleanup() {
    let mut cloud = FakeCloud::start().await;
    let connection = established_connection(&mut cloud).await;

    let shutdown = connection.shutdown_with_grace(Duration::from_secs(1));
    drop(shutdown);
    cloud.wait_for_disconnect().await;
}
