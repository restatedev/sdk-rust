//! Protocol-level tests driving a handler through [`Endpoint::handle`], without a Restate server.
//!
//! They cover awaiting while the input is closed and a `ctx.run()` closure is still executing:
//! depending on the shared-core version, the VM either suspends right away, or first waits for
//! the run's completion to be proposed (`WaitingExternalProgress { waiting_input: false, .. }`).
//! Either way the SDK must neither panic nor lose a completed run's result.

use bytes::{Buf, Bytes, BytesMut};
use futures::StreamExt;
use futures::channel::mpsc;
use http_body::Frame;
use http_body_util::{BodyExt, StreamBody};
use restate_sdk::endpoint::Endpoint;
use restate_sdk::prelude::*;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::Notify;

// --- Protocol messages

const START: u16 = 0x0000;
const SUSPENSION: u16 = 0x0001;
const ERROR: u16 = 0x0002;
const END: u16 = 0x0003;
const PROPOSE_RUN_COMPLETION: u16 = 0x0005;
const INPUT_COMMAND: u16 = 0x0400;
const SLEEP_COMMAND: u16 = 0x040C;
const RUN_COMMAND: u16 = 0x0411;

fn encode_message(ty: u16, body: &[u8]) -> Bytes {
    let header = (u64::from(ty) << 48) | body.len() as u64;
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&header.to_be_bytes());
    buf.extend_from_slice(body);
    buf.into()
}

// Just enough protobuf encoding to write StartMessage and InputCommandMessage.
fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push(v as u8 | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn put_varint_field(buf: &mut Vec<u8>, field: u32, v: u64) {
    put_varint(buf, u64::from(field << 3));
    put_varint(buf, v);
}

fn put_bytes_field(buf: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_varint(buf, u64::from((field << 3) | 2));
    put_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn start_message(known_entries: u32) -> Bytes {
    let mut body = vec![];
    put_bytes_field(&mut body, 1, b"invocation-id"); // id
    put_bytes_field(&mut body, 2, b"inv_1"); // debug_id
    put_varint_field(&mut body, 3, u64::from(known_entries));
    put_varint_field(&mut body, 9, 1); // random_seed
    encode_message(START, &body)
}

fn input_command(input: &[u8]) -> Bytes {
    let mut value = vec![];
    put_bytes_field(&mut value, 1, input); // Value.content
    let mut body = vec![];
    put_bytes_field(&mut body, 14, &value); // value
    encode_message(INPUT_COMMAND, &body)
}

// --- Harness

/// An invocation driven through [`Endpoint::handle`]: the test writes the request body and reads
/// the response body as protocol message types.
struct Invocation {
    input_tx: Option<mpsc::UnboundedSender<Bytes>>,
    /// Notified when the SDK reads the end of the request body.
    input_end_read: Arc<Notify>,
    output_rx: tokio::sync::mpsc::UnboundedReceiver<u16>,
    driver: tokio::task::JoinHandle<()>,
}

impl Invocation {
    fn start(endpoint: &Endpoint, service: &str, handler: &str) -> Self {
        let (input_tx, mut input_rx) = mpsc::unbounded::<Bytes>();
        let input_end_read = Arc::new(Notify::new());
        let end_read = Arc::clone(&input_end_read);
        let request_body = StreamBody::new(futures::stream::poll_fn(move |cx| {
            match input_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(b)) => Poll::Ready(Some(Ok::<_, Infallible>(Frame::data(b)))),
                Poll::Ready(None) => {
                    end_read.notify_one();
                    Poll::Ready(None)
                }
                Poll::Pending => Poll::Pending,
            }
        }));
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/invoke/{service}/{handler}"))
            .header(
                http::header::CONTENT_TYPE,
                "application/vnd.restate.invocation.v7",
            )
            .body(request_body)
            .unwrap();

        let response = endpoint.handle(request);
        assert_eq!(response.status(), 200);

        // Polling the response body is what drives the handler.
        let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut response_body = response.into_body();
        let driver = tokio::spawn(async move {
            let mut buf = BytesMut::new();
            while let Some(frame) = response_body.frame().await {
                let Ok(data) = frame.unwrap().into_data() else {
                    continue;
                };
                buf.extend_from_slice(&data);
                while buf.len() >= 8 {
                    let header = u64::from_be_bytes(buf[..8].try_into().unwrap());
                    let len = header as u32 as usize;
                    if buf.len() < 8 + len {
                        break;
                    }
                    buf.advance(8 + len);
                    let _ = output_tx.send((header >> 48) as u16);
                }
            }
        });

        Self {
            input_tx: Some(input_tx),
            input_end_read,
            output_rx,
            driver,
        }
    }

    fn send(&self, message: Bytes) {
        self.input_tx
            .as_ref()
            .unwrap()
            .unbounded_send(message)
            .unwrap();
    }

    /// Closes the request body, and waits until the SDK has read its end.
    async fn close_input(&mut self) {
        self.input_tx.take();
        self.input_end_read.notified().await;
    }

    async fn next_message(&mut self) -> Option<u16> {
        self.output_rx.recv().await
    }

    /// Reads the response until it ends, failing if the SDK panicked.
    async fn finish(mut self, mut messages: Vec<u16>) -> Vec<u16> {
        while let Some(ty) = self.output_rx.recv().await {
            messages.push(ty);
        }
        if let Err(e) = self.driver.await {
            panic!(
                "the SDK panicked while handling the invocation: {e}; output so far: {messages:x?}"
            );
        }
        messages
    }
}

// --- Service

#[derive(Default)]
struct RunGate {
    started: Notify,
    release: Notify,
    finished: AtomicBool,
}

impl RunGate {
    async fn run(&self) -> HandlerResult<String> {
        self.started.notify_one();
        self.release.notified().await;
        self.finished.store(true, Ordering::SeqCst);
        Ok("done".to_owned())
    }
}

struct AwaitWhileRunning(Arc<RunGate>);

#[restate_sdk::service(name = "AwaitWhileRunning")]
impl AwaitWhileRunning {
    #[handler(name = "joinSleepAndRun")]
    async fn join_sleep_and_run(&self, ctx: Context<'_>) -> HandlerResult<()> {
        let gate = Arc::clone(&self.0);
        let (slept, ran) = tokio::join!(
            ctx.sleep(Duration::from_secs(3600)),
            ctx.run(|| async move { gate.run().await }),
        );
        slept?;
        ran?;
        Ok(())
    }

    #[handler(name = "joinSelectAndRun")]
    async fn join_select_and_run(&self, ctx: Context<'_>) -> HandlerResult<()> {
        let gate = Arc::clone(&self.0);
        let (selected, ran) = tokio::join!(
            async {
                restate_sdk::select! {
                    res = ctx.sleep(Duration::from_secs(3600)) => res,
                    res = ctx.sleep(Duration::from_secs(7200)) => res,
                }
            },
            ctx.run(|| async move { gate.run().await }),
        );
        selected?;
        ran?;
        Ok(())
    }
}

/// Starts `handler`, which awaits something else while its run closure executes, then closes the
/// input while the run is executing, and only then lets the run complete.
async fn close_input_while_run_executes(handler: &str) {
    let gate = Arc::new(RunGate::default());
    let endpoint = Endpoint::builder()
        .bind(AwaitWhileRunning(Arc::clone(&gate)))
        .build();

    let mut invocation = Invocation::start(&endpoint, "AwaitWhileRunning", handler);
    invocation.send(start_message(1));
    invocation.send(input_command(b""));

    gate.started.notified().await;
    let mut messages = vec![];
    while !messages.contains(&RUN_COMMAND) {
        messages.push(
            invocation
                .next_message()
                .await
                .expect("response ended early"),
        );
    }

    invocation.close_input().await;
    gate.release.notify_one();
    let messages = invocation.finish(messages).await;

    assert!(messages.contains(&SLEEP_COMMAND), "{messages:x?}");
    assert!(
        !messages.contains(&ERROR) && !messages.contains(&END),
        "{messages:x?}"
    );
    assert_eq!(messages.last(), Some(&SUSPENSION), "{messages:x?}");
    if gate.finished.load(Ordering::SeqCst) {
        // The VM waited for the run before suspending: its result must be proposed (before the
        // suspension, which is the last message), not lost.
        assert_eq!(
            messages
                .iter()
                .filter(|&&ty| ty == PROPOSE_RUN_COMPLETION)
                .count(),
            1,
            "{messages:x?}"
        );
    } else {
        // The VM suspended right away, dropping the run before it completed.
        assert!(!messages.contains(&PROPOSE_RUN_COMPLETION), "{messages:x?}");
    }
}

#[tokio::test]
async fn join_sleep_and_run_with_input_closed_while_run_executes() {
    tokio::time::timeout(
        Duration::from_secs(10),
        close_input_while_run_executes("joinSleepAndRun"),
    )
    .await
    .expect("the invocation hung");
}

#[tokio::test]
async fn join_select_and_run_with_input_closed_while_run_executes() {
    tokio::time::timeout(
        Duration::from_secs(10),
        close_input_while_run_executes("joinSelectAndRun"),
    )
    .await
    .expect("the invocation hung");
}
