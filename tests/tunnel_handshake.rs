#![cfg(feature = "tunnel")]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use ed25519_dalek::SigningKey;
use http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use restate_sdk::prelude::{Endpoint, Tunnel};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::task::AbortOnDropHandle;

const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const IDENTITY_SEED: [u8; 32] = [0x3b; 32];

type CloudBody = UnsyncBoxBody<Bytes, Infallible>;

#[derive(Clone, Copy)]
enum HandshakeResult {
    Unauthorized,
    TooManyTunnels,
    UnexpectedData,
    Ok,
}

impl HandshakeResult {
    fn trailers(self) -> HeaderMap {
        let mut trailers = HeaderMap::new();
        match self {
            Self::Unauthorized => {
                trailers.insert("tunnel-status", "unauthorized".parse().unwrap());
            }
            Self::TooManyTunnels => {
                trailers.insert("tunnel-status", "too-many-tunnels".parse().unwrap());
            }
            Self::UnexpectedData => unreachable!("DATA attempts do not send trailers"),
            Self::Ok => {
                trailers.insert("tunnel-status", "ok".parse().unwrap());
                trailers.insert("tunnel-name", "taxonomy-test".parse().unwrap());
                trailers.insert(
                    "proxy-url",
                    "https://proxy.example/env_taxonomy/taxonomy-test"
                        .parse()
                        .unwrap(),
                );
                trailers.insert(
                    "tunnel-url",
                    "https://tunnel.example/taxonomy-test".parse().unwrap(),
                );
            }
        }
        trailers
    }
}

struct HandshakeObservation {
    status: StatusCode,
    credentials: HeaderMap,
}

/// A scripted role-reversed H2 peer which owns every connection driver it
/// starts. Each script entry applies to one newly accepted SDK dial.
struct ScriptedCloud {
    address: SocketAddr,
    observations: mpsc::Receiver<HandshakeObservation>,
    task: Option<JoinHandle<()>>,
}

impl ScriptedCloud {
    async fn start(script: Vec<HandshakeResult>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (observation_tx, observations) = mpsc::channel(script.len());

        let task = tokio::spawn(async move {
            for result in script {
                let (socket, _) = tokio::time::timeout(TEST_TIMEOUT, listener.accept())
                    .await
                    .expect("SDK did not dial the scripted Cloud peer")
                    .unwrap();
                let (mut sender, connection) =
                    hyper::client::conn::http2::handshake::<_, _, CloudBody>(
                        TokioExecutor::new(),
                        TokioIo::new(socket),
                    )
                    .await
                    .unwrap();
                // If this outer script task is aborted, the driver is aborted
                // too instead of becoming a detached Tokio task.
                let connection = AbortOnDropHandle::new(tokio::spawn(connection));

                let (trailers_tx, body) = if matches!(result, HandshakeResult::UnexpectedData) {
                    // More than one maximum-sized H2 frame proves the SDK
                    // rejects the first DATA frame instead of collecting the
                    // handshake request body without a bound.
                    (
                        None,
                        Full::new(Bytes::from(vec![b'x'; 128 * 1024])).boxed_unsync(),
                    )
                } else {
                    let (trailers_tx, trailers_rx) = oneshot::channel();
                    let body = Empty::<Bytes>::new()
                        .with_trailers(
                            async move { trailers_rx.await.ok().map(Ok::<_, Infallible>) },
                        )
                        .boxed_unsync();
                    (Some(trailers_tx), body)
                };
                let request = Request::builder()
                    .method(Method::GET)
                    .uri("http://fake-tunnel.test/_/start-tunnel")
                    .body(body)
                    .unwrap();
                let response = sender.send_request(request).await.unwrap();
                observation_tx
                    .send(HandshakeObservation {
                        status: response.status(),
                        credentials: response.headers().clone(),
                    })
                    .await
                    .unwrap();
                if let Some(trailers_tx) = trailers_tx {
                    trailers_tx.send(result.trailers()).unwrap();
                }

                // A rejected attempt must close before the slot redials; the
                // successful final attempt remains here until the test closes
                // its TunnelConnection. Keeping `sender` alive ensures the
                // close is initiated by the SDK rather than this fake peer.
                let _driver_result = tokio::time::timeout(TEST_TIMEOUT, connection)
                    .await
                    .expect("SDK did not close a completed handshake attempt")
                    .expect("fake Cloud H2 driver task panicked");
                // Closing a rejected attempt can race queued trailer DATA and
                // surface BrokenPipe (or another transport error) in Hyper's
                // client driver. Driver completion is what matters here.
                drop(sender);
            }
        });

        Self {
            address,
            observations,
            task: Some(task),
        }
    }

    async fn observation(&mut self) -> HandshakeObservation {
        tokio::time::timeout(TEST_TIMEOUT, self.observations.recv())
            .await
            .expect("SDK did not answer the scripted start request")
            .expect("script ended before the expected start response")
    }

    async fn finish(&mut self) {
        let task = AbortOnDropHandle::new(self.task.take().expect("script task awaited once"));
        tokio::time::timeout(TEST_TIMEOUT, task)
            .await
            .expect("scripted Cloud peer did not finish")
            .expect("scripted Cloud peer task panicked");
    }
}

impl Drop for ScriptedCloud {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn tunnel(address: SocketAddr) -> Tunnel {
    let key = SigningKey::from_bytes(&IDENTITY_SEED);
    let public_key = format!(
        "publickeyv1_{}",
        bs58::encode(key.verifying_key().to_bytes()).into_string()
    );
    Tunnel::new(Endpoint::builder().build())
        .tunnel_servers([format!("http://{address}")])
        .environment_id("env_taxonomy")
        .auth_token("test-token")
        .signing_public_key(public_key)
        .tunnel_name("taxonomy-test")
        .tunnel_worker_id("worker-taxonomy")
}

fn assert_start_response(observation: &HandshakeObservation) {
    assert_eq!(observation.status, StatusCode::OK);
    assert_eq!(
        observation.credentials["authorization"],
        "Bearer test-token"
    );
    assert_eq!(observation.credentials["environment-id"], "env_taxonomy");
    assert_eq!(observation.credentials["tunnel-name"], "taxonomy-test");
}

#[tokio::test]
async fn unauthorized_is_fatal_before_readiness() {
    let mut cloud = ScriptedCloud::start(vec![HandshakeResult::Unauthorized]).await;
    let connect = AbortOnDropHandle::new(tokio::spawn(tunnel(cloud.address).connect()));

    let observation = cloud.observation().await;
    assert_start_response(&observation);

    let result = tokio::time::timeout(TEST_TIMEOUT, connect)
        .await
        .expect("fatal handshake did not terminate connect")
        .expect("connect task panicked");
    let error = match result {
        Ok(connection) => {
            connection.close().await.unwrap();
            panic!("unauthorized handshake unexpectedly established");
        }
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("unauthorized"),
        "fatal error should retain the non-secret status: {error}"
    );
    cloud.finish().await;
}

#[tokio::test]
async fn too_many_tunnels_retries_and_later_success_establishes() {
    let mut cloud =
        ScriptedCloud::start(vec![HandshakeResult::TooManyTunnels, HandshakeResult::Ok]).await;
    let connect = AbortOnDropHandle::new(tokio::spawn(tunnel(cloud.address).connect()));

    let first = cloud.observation().await;
    let first_connection_id = first.credentials["tunnel-connection-id"].clone();
    assert_start_response(&first);
    assert!(
        !connect.is_finished(),
        "a retryable Cloud status terminated connect"
    );

    let second = cloud.observation().await;
    assert_start_response(&second);
    assert_ne!(
        first_connection_id, second.credentials["tunnel-connection-id"],
        "each redial must use a fresh connection ID"
    );

    let connection = tokio::time::timeout(TEST_TIMEOUT, connect)
        .await
        .expect("successful retry did not establish")
        .expect("connect task panicked")
        .expect("successful trailers returned an error");
    assert_eq!(connection.info().tunnel_name(), "taxonomy-test");

    connection.close().await.unwrap();
    cloud.finish().await;
}

#[tokio::test]
async fn unexpected_handshake_data_is_rejected_without_collection_and_retried() {
    let mut cloud =
        ScriptedCloud::start(vec![HandshakeResult::UnexpectedData, HandshakeResult::Ok]).await;
    let connect = AbortOnDropHandle::new(tokio::spawn(tunnel(cloud.address).connect()));

    let first = cloud.observation().await;
    let first_connection_id = first.credentials["tunnel-connection-id"].clone();
    assert_start_response(&first);

    let second = cloud.observation().await;
    assert_start_response(&second);
    assert_ne!(
        first_connection_id, second.credentials["tunnel-connection-id"],
        "unexpected handshake DATA must reject the attempt and redial"
    );

    let connection = tokio::time::timeout(TEST_TIMEOUT, connect)
        .await
        .expect("retry after unexpected DATA did not establish")
        .expect("connect task panicked")
        .expect("successful trailers returned an error");
    connection.close().await.unwrap();
    cloud.finish().await;
}
