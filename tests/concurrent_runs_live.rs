#![cfg(feature = "http_server")]

//! Real-server acceptance for concurrent durable runs. Run with
//! `RESTATE_SERVER_BIN=/path/to/restate-server cargo test --test concurrent_runs_live -- --ignored --nocapture`.
//! Each test owns an isolated server, its state directory, and loopback listeners.

use std::convert::Infallible;
use std::future::{Ready, ready};
use std::net::TcpListener as ReservedListener;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use futures::task::AtomicWaker;
use http_body::{Body, Frame};
use http_body_util::{BodyExt, combinators::UnsyncBoxBody};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use restate_sdk::endpoint::{HandleOptions, ProtocolMode};
use restate_sdk::prelude::*;
use tokio::sync::{Notify, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

const TIMEOUT: Duration = Duration::from_secs(30);
static NEXT_SERVER: AtomicUsize = AtomicUsize::new(0);

struct Observations {
    attempts: AtomicUsize,
    executions: [AtomicUsize; 3],
    dropped: [AtomicUsize; 3],
    releases: [Semaphore; 3],
    completion_ids: [AtomicU32; 3],
    stored: [AtomicUsize; 3],
    completion_order: Mutex<Vec<usize>>,
    proposals: AtomicUsize,
    errors: Mutex<Vec<u32>>,
    crash_after_two: AtomicBool,
    crash: AtomicBool,
    response_waker: AtomicWaker,
    changed: Notify,
}

impl Observations {
    fn new(crash: bool) -> Arc<Self> {
        Arc::new(Self {
            attempts: AtomicUsize::new(0),
            executions: std::array::from_fn(|_| AtomicUsize::new(0)),
            dropped: std::array::from_fn(|_| AtomicUsize::new(0)),
            releases: std::array::from_fn(|_| Semaphore::new(0)),
            completion_ids: std::array::from_fn(|_| AtomicU32::new(u32::MAX)),
            stored: std::array::from_fn(|_| AtomicUsize::new(0)),
            completion_order: Mutex::new(Vec::new()),
            proposals: AtomicUsize::new(0),
            errors: Mutex::new(Vec::new()),
            crash_after_two: AtomicBool::new(crash),
            crash: AtomicBool::new(false),
            response_waker: AtomicWaker::new(),
            changed: Notify::new(),
        })
    }

    fn counts(&self) -> [usize; 3] {
        self.executions.each_ref().map(|n| n.load(Ordering::SeqCst))
    }

    async fn until(&self, description: &str, predicate: impl Fn() -> bool) {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let changed = self.changed.notified();
                if predicate() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out {description}: attempts={} counts={:?} errors={:?}",
                self.attempts.load(Ordering::SeqCst),
                self.counts(),
                self.errors.lock().unwrap()
            )
        });
    }

    async fn execute(self: Arc<Self>, index: usize) -> HandlerResult<u32> {
        let mut lifetime = RunLifetime {
            observations: self.clone(),
            index,
            finished: false,
        };
        self.executions[index].fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        self.releases[index].acquire().await.unwrap().forget();
        self.completion_order.lock().unwrap().push(index);
        lifetime.finished = true;
        Ok(index as u32)
    }

    async fn execute_progressive(
        self: Arc<Self>,
        index: usize,
        attempt: usize,
    ) -> HandlerResult<u32> {
        let mut lifetime = RunLifetime {
            observations: self.clone(),
            index,
            finished: false,
        };
        self.executions[index].fetch_add(1, Ordering::SeqCst);
        self.changed.notify_one();
        self.releases[index].acquire().await.unwrap().forget();
        lifetime.finished = true;
        if (index, attempt) == (0, 1) || (index, attempt) == (1, 2) {
            return Err(
                std::io::Error::other(format!("run {index} failed on attempt {attempt}")).into(),
            );
        }
        self.completion_order.lock().unwrap().push(index);
        Ok(index as u32)
    }

    fn received(&self, ty: u16, payload: &[u8]) {
        // V6 returns the stored RunCompletionNotification; V7 returns a
        // ProposeRunCompletionAck after storage and replication. Replay always
        // carries RunCompletionNotification in its original notification order.
        if !matches!(ty, 0x8011 | 0x0007) {
            return;
        }
        let id = protobuf_uint(payload, 1).unwrap_or(0) as u32;
        for index in 0..3 {
            if self.completion_ids[index].load(Ordering::SeqCst) == id {
                self.stored[index].fetch_add(1, Ordering::SeqCst);
                if index == 2 && self.crash_after_two.swap(false, Ordering::SeqCst) {
                    self.crash.store(true, Ordering::SeqCst);
                    self.response_waker.wake();
                }
                self.changed.notify_one();
            }
        }
    }

    fn sent(&self, ty: u16, payload: &[u8]) {
        if ty == 0x0005 {
            self.proposals.fetch_add(1, Ordering::SeqCst);
        }
        if ty == 0x0002 {
            self.errors
                .lock()
                .unwrap()
                .push(protobuf_uint(payload, 1).unwrap_or(0) as u32);
            self.changed.notify_one();
        }
        if ty == 0x0411 {
            let name = protobuf_bytes(payload, 12).unwrap_or_default();
            for index in 0..3 {
                if name == format!("run-{index}").as_bytes() {
                    self.completion_ids[index].store(
                        protobuf_uint(payload, 11).unwrap_or(0) as u32,
                        Ordering::SeqCst,
                    );
                }
            }
        }
    }
}

struct RunLifetime {
    observations: Arc<Observations>,
    index: usize,
    finished: bool,
}

impl Drop for RunLifetime {
    fn drop(&mut self) {
        if !self.finished {
            self.observations.dropped[self.index].fetch_add(1, Ordering::SeqCst);
            self.observations.changed.notify_one();
        }
    }
}

struct ConcurrentRuns(Arc<Observations>);

#[service]
impl ConcurrentRuns {
    #[handler(invocation_retry_policy(initial_interval = "10ms", max_attempts = 4))]
    async fn runs(&self, ctx: Context<'_>) -> HandlerResult<Json<Vec<u32>>> {
        self.0.attempts.fetch_add(1, Ordering::SeqCst);
        self.0.changed.notify_one();
        let run = |index| {
            let observations = self.0.clone();
            ctx.run(move || observations.execute(index))
                .name(format!("run-{index}"))
                .start()
        };
        let first = run(0);
        let second = run(1);
        let third = run(2);
        let (first, second, third) = tokio::join!(first, second, third);
        Ok(Json(vec![first?, second?, third?]))
    }
}

struct ProgressiveRuns(Arc<Observations>);

#[service]
impl ProgressiveRuns {
    #[handler(invocation_retry_policy(initial_interval = "10ms", max_attempts = 4))]
    async fn runs(&self, ctx: Context<'_>) -> HandlerResult<Json<Vec<u32>>> {
        let attempt = self.0.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.changed.notify_one();
        let run = |index| {
            let observations = self.0.clone();
            ctx.run(move || observations.execute_progressive(index, attempt))
                .name(format!("run-{index}"))
                .retry_policy(RunRetryPolicy::default().initial_delay(Duration::from_millis(10)))
                .start()
        };
        let first = run(0);
        let second = run(1);
        let (first, second) = tokio::join!(first, second);
        Ok(Json(vec![first?, second?]))
    }
}

#[derive(Default)]
struct ProtocolFrames(BytesMut);

impl ProtocolFrames {
    fn push(&mut self, data: &[u8], mut observe: impl FnMut(u16, &[u8])) {
        self.0.extend_from_slice(data);
        while self.0.len() >= 8 {
            let ty = u16::from_be_bytes(self.0[..2].try_into().unwrap());
            let length = u32::from_be_bytes(self.0[4..8].try_into().unwrap()) as usize;
            if self.0.len() < 8 + length {
                return;
            }
            observe(ty, &self.0[8..8 + length]);
            self.0.advance(8 + length);
        }
    }
}

fn varint(data: &mut &[u8]) -> u64 {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = data[0];
        *data = &data[1..];
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("invalid protocol varint");
}

fn protobuf_field(mut data: &[u8], wanted: u64) -> Option<(u64, &[u8])> {
    while !data.is_empty() {
        let key = varint(&mut data);
        let field = match key & 7 {
            0 => {
                let before = data;
                varint(&mut data);
                &before[..before.len() - data.len()]
            }
            1 => {
                let field = &data[..8];
                data = &data[8..];
                field
            }
            2 => {
                let length = varint(&mut data) as usize;
                let field = &data[..length];
                data = &data[length..];
                field
            }
            5 => {
                let field = &data[..4];
                data = &data[4..];
                field
            }
            wire => panic!("unexpected protocol protobuf wire type {wire}"),
        };
        if key >> 3 == wanted {
            return Some((key & 7, field));
        }
    }
    None
}

fn protobuf_uint(data: &[u8], field: u64) -> Option<u64> {
    protobuf_field(data, field).map(|(wire, mut value)| {
        assert_eq!(wire, 0);
        varint(&mut value)
    })
}

fn protobuf_bytes(data: &[u8], field: u64) -> Option<&[u8]> {
    protobuf_field(data, field).map(|(wire, value)| {
        assert_eq!(wire, 2);
        value
    })
}

struct InputTap {
    inner: Incoming,
    frames: ProtocolFrames,
    observations: Arc<Observations>,
}

impl Body for InputTap {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let frame = std::task::ready!(Pin::new(&mut self.inner).poll_frame(cx));
        if let Some(Ok(frame)) = &frame
            && let Some(data) = frame.data_ref()
        {
            let observations = self.observations.clone();
            self.frames
                .push(data, |ty, payload| observations.received(ty, payload));
        }
        Poll::Ready(frame)
    }
}

struct OutputTap {
    inner: UnsyncBoxBody<Bytes, std::io::Error>,
    frames: ProtocolFrames,
    observations: Arc<Observations>,
    invocation: bool,
}

impl Body for OutputTap {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        if self.invocation {
            self.observations.response_waker.register(cx.waker());
            if self.observations.crash.swap(false, Ordering::SeqCst) {
                return Poll::Ready(Some(Err(std::io::Error::other(
                    "injected response-stream failure after run 2 was stored",
                ))));
            }
        }
        let frame = std::task::ready!(Pin::new(&mut self.inner).poll_frame(cx));
        if self.invocation
            && let Some(Ok(frame)) = &frame
            && let Some(data) = frame.data_ref()
        {
            let observations = self.observations.clone();
            self.frames
                .push(data, |ty, payload| observations.sent(ty, payload));
        }
        Poll::Ready(frame)
    }
}

#[derive(Clone)]
struct TappedEndpoint {
    endpoint: Endpoint,
    observations: Arc<Observations>,
}

impl hyper::service::Service<http::Request<Incoming>> for TappedEndpoint {
    type Response = http::Response<OutputTap>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn call(&self, request: http::Request<Incoming>) -> Self::Future {
        let invocation = request.uri().path().starts_with("/invoke/");
        let request = request.map(|inner| InputTap {
            inner,
            frames: ProtocolFrames::default(),
            observations: self.observations.clone(),
        });
        let response = self.endpoint.handle_with_options(
            request,
            HandleOptions {
                protocol_mode: ProtocolMode::BidiStream,
            },
        );
        if invocation {
            println!(
                "live invocation protocol: {:?}",
                response.headers().get("content-type")
            );
        }
        ready(Ok(response.map(|inner| OutputTap {
            inner: inner
                .map_err(|error| std::io::Error::other(error.to_string()))
                .boxed_unsync(),
            frames: ProtocolFrames::default(),
            observations: self.observations.clone(),
            invocation,
        })))
    }
}

struct PrivateEndpoint {
    url: String,
    task: JoinHandle<()>,
}

impl PrivateEndpoint {
    async fn start(endpoint: Endpoint, observations: Arc<Observations>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (socket, _) = connection.unwrap();
                        let service = TappedEndpoint { endpoint: endpoint.clone(), observations: observations.clone() };
                        connections.spawn(async move {
                            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                                .serve_connection(TokioIo::new(socket), service)
                                .await;
                        });
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self { url, task }
    }
}

impl Drop for PrivateEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct PrivateServer {
    child: Child,
    directory: PathBuf,
    ingress: String,
    admin: String,
}

impl PrivateServer {
    async fn start(client: &reqwest::Client) -> Self {
        let binary = std::env::var_os("RESTATE_SERVER_BIN")
            .expect("set RESTATE_SERVER_BIN to run the ignored live acceptance tests");
        let version = Command::new(&binary).arg("--version").output().unwrap();
        assert!(version.status.success());
        println!(
            "live server: {}",
            String::from_utf8_lossy(&version.stdout).trim()
        );
        let directory = std::env::temp_dir().join(format!(
            "sdk-concurrent-runs-{}-{}",
            std::process::id(),
            NEXT_SERVER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&directory).unwrap();
        let reservations: [ReservedListener; 3] =
            std::array::from_fn(|_| ReservedListener::bind("127.0.0.1:0").unwrap());
        let addresses = reservations
            .each_ref()
            .map(|listener| listener.local_addr().unwrap());
        let log = std::fs::File::create(directory.join("server.log")).unwrap();
        let mut command = Command::new(binary);
        command
            .arg("--no-logo")
            .env_clear()
            .env("RESTATE_BASE_DIR", directory.join("data"))
            .env("RESTATE_NODE_NAME", "concurrent-runs")
            .env("RESTATE_CLUSTER_NAME", "concurrent-runs")
            .env("RESTATE_BIND_ADDRESS", addresses[2].to_string())
            .env("RESTATE_INGRESS__BIND_ADDRESS", addresses[0].to_string())
            .env("RESTATE_ADMIN__BIND_ADDRESS", addresses[1].to_string())
            .env("RESTATE_BIND_IP", "127.0.0.1")
            .env("RESTATE_LISTEN_MODE", "tcp")
            .env("RESTATE_DEFAULT_NUM_PARTITIONS", "1")
            .env("RESTATE_ROCKSDB_TOTAL_MEMORY_SIZE", "256MB")
            .env("RESTATE_LOG_FILTER", "warn,restate=info")
            .env("RESTATE_LOG_FORMAT", "compact")
            .env("RESTATE_LOG_DISABLE_ANSI_CODES", "true")
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        drop(reservations);
        let mut server = Self {
            child: command.spawn().unwrap(),
            directory,
            ingress: format!("http://{}", addresses[0]),
            admin: format!("http://{}", addresses[1]),
        };
        tokio::time::timeout(TIMEOUT, async {
            loop {
                assert!(
                    server.child.try_wait().unwrap().is_none(),
                    "private Restate exited"
                );
                if let Ok(response) = client
                    .post(format!("{}/query", server.admin))
                    .json(&serde_json::json!({"query": "SELECT id FROM sys_invocation LIMIT 1"}))
                    .send()
                    .await
                    && response.status().is_success()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("private Restate did not become ready");
        server
    }
}

impl Drop for PrivateServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let log = self.directory.join("server.log");
        if std::thread::panicking() {
            eprintln!(
                "private Restate log:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        if let Some(destination) = std::env::var_os("RESTATE_LIVE_LOG_DIR") {
            let destination = PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            let _ = std::fs::copy(
                &log,
                destination
                    .join(self.directory.file_name().unwrap())
                    .with_extension("log"),
            );
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn register(client: &reqwest::Client, server: &PrivateServer, endpoint: &PrivateEndpoint) {
    let response = client
        .post(format!("{}/deployments", server.admin))
        .json(&serde_json::json!({"uri": endpoint.url}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(status.is_success(), "register endpoint: {status} {body}");
}

async fn concurrent_case(crash: bool) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(TIMEOUT)
        .build()
        .unwrap();
    let server = PrivateServer::start(&client).await;
    let observations = Observations::new(crash);
    let endpoint = PrivateEndpoint::start(
        Endpoint::builder()
            .bind(ConcurrentRuns(observations.clone()))
            .build(),
        observations.clone(),
    )
    .await;
    register(&client, &server, &endpoint).await;
    let invocation = client
        .post(format!("{}/ConcurrentRuns/runs", server.ingress))
        .send();
    let control = async {
        observations
            .until("all three closures start", || {
                observations.counts() == [1, 1, 1]
            })
            .await;
        observations.releases[2].add_permits(1);
        observations
            .until("run 2 is durably stored", || {
                observations.stored[2].load(Ordering::SeqCst) > 0
            })
            .await;
        if crash {
            observations
                .until("unfinished runs drop on stream failure", || {
                    observations.dropped[0].load(Ordering::SeqCst) == 1
                        && observations.dropped[1].load(Ordering::SeqCst) == 1
                })
                .await;
            observations
                .until("partial replay starts only unfinished runs", || {
                    observations.counts() == [2, 2, 1]
                })
                .await;
        }
        observations.releases[0].add_permits(1);
        observations
            .until("run 0 is durably stored", || {
                observations.stored[0].load(Ordering::SeqCst) > 0
            })
            .await;
        observations.releases[1].add_permits(1);
    };
    let (response, ()) = tokio::join!(invocation, control);
    let response = response.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status.as_u16(), 200, "{body}");
    assert_eq!(body, "[0,1,2]");
    assert_eq!(
        observations.counts(),
        if crash { [2, 2, 1] } else { [1, 1, 1] }
    );
    assert_eq!(
        observations.attempts.load(Ordering::SeqCst),
        if crash { 2 } else { 1 }
    );
    assert_eq!(*observations.completion_order.lock().unwrap(), [2, 0, 1]);
    assert!(!observations.errors.lock().unwrap().contains(&570));
    assert!(observations.errors.lock().unwrap().is_empty());
    println!(
        "live concurrent runs crash={crash}: results={body} counts={:?} completion_order=[2,0,1] no570",
        observations.counts()
    );
}

#[tokio::test]
#[ignore = "requires RESTATE_SERVER_BIN; owns a private Restate server"]
async fn concurrent_runs_start_together_and_complete_in_shuffled_order() {
    concurrent_case(false).await;
}

#[tokio::test]
#[ignore = "requires RESTATE_SERVER_BIN; owns a private Restate server"]
async fn concurrent_runs_partial_replay_skips_stored_closure_after_stream_failure() {
    concurrent_case(true).await;
}

#[tokio::test]
#[ignore = "requires RESTATE_SERVER_BIN; owns a private Restate server"]
async fn concurrent_runs_progressive_retries_replay_only_stored_results() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(TIMEOUT)
        .build()
        .unwrap();
    let server = PrivateServer::start(&client).await;
    let observations = Observations::new(false);
    let endpoint = PrivateEndpoint::start(
        Endpoint::builder()
            .bind(ProgressiveRuns(observations.clone()))
            .build(),
        observations.clone(),
    )
    .await;
    register(&client, &server, &endpoint).await;
    let invocation = client
        .post(format!("{}/ProgressiveRuns/runs", server.ingress))
        .send();
    let control = async {
        observations
            .until("both runs start on attempt 1", || {
                observations.counts() == [1, 1, 0]
            })
            .await;
        observations.releases[0].add_permits(1);
        observations
            .until("failed attempt drops unfinished run 1", || {
                observations.dropped[1].load(Ordering::SeqCst) == 1
            })
            .await;
        observations
            .until("both runs start on attempt 2", || {
                observations.counts() == [2, 2, 0]
            })
            .await;
        observations.releases[0].add_permits(1);
        observations
            .until("run 0 is stored before run 1 fails", || {
                observations.stored[0].load(Ordering::SeqCst) > 0
            })
            .await;
        observations.releases[1].add_permits(1);
        observations
            .until("attempt 3 starts only unfinished run 1", || {
                observations.counts() == [2, 3, 0]
            })
            .await;
        observations.releases[1].add_permits(1);
    };
    let (response, ()) = tokio::join!(invocation, control);
    let response = response.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status.as_u16(), 200, "{body}");
    assert_eq!(body, "[0,1]");
    assert_eq!(observations.counts(), [2, 3, 0]);
    assert_eq!(observations.attempts.load(Ordering::SeqCst), 3);
    assert_eq!(*observations.completion_order.lock().unwrap(), [0, 1]);
    assert_eq!(*observations.errors.lock().unwrap(), [500, 500]);
    println!(
        "live progressive retries: results={body} attempts=3 counts=[2,3] errors=[500,500] no570"
    );
}

#[tokio::test]
#[ignore = "requires RESTATE_SERVER_BIN; owns a private Restate server"]
async fn concurrent_runs_cancellation_drops_all_pending_closures() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(TIMEOUT)
        .build()
        .unwrap();
    let server = PrivateServer::start(&client).await;
    let observations = Observations::new(false);
    let endpoint = PrivateEndpoint::start(
        Endpoint::builder()
            .bind(ConcurrentRuns(observations.clone()))
            .build(),
        observations.clone(),
    )
    .await;
    register(&client, &server, &endpoint).await;
    let response = client
        .post(format!("{}/ConcurrentRuns/runs/send", server.ingress))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body = response.json::<serde_json::Value>().await.unwrap();
    let invocation = body["invocationId"].as_str().unwrap();
    observations
        .until("all closures are pending before cancellation", || {
            observations.counts() == [1, 1, 1]
        })
        .await;
    let response = client
        .patch(format!("{}/invocations/{invocation}/cancel", server.admin))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    observations
        .until("cancellation drops every pending closure", || {
            observations
                .dropped
                .each_ref()
                .map(|n| n.load(Ordering::SeqCst))
                == [1, 1, 1]
        })
        .await;
    let response = client
        .get(format!(
            "{}/restate/invocation/{invocation}/attach",
            server.ingress
        ))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status.as_u16(), 409, "{body}");
    assert_eq!(observations.counts(), [1, 1, 1]);
    assert_eq!(observations.attempts.load(Ordering::SeqCst), 1);
    assert!(observations.completion_order.lock().unwrap().is_empty());
    assert_eq!(observations.proposals.load(Ordering::SeqCst), 0);
    assert!(observations.errors.lock().unwrap().is_empty());
    println!("live cancellation: terminal409 counts=[1,1,1] drops=[1,1,1] no proposals no570");
}
