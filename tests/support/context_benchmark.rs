#[allow(dead_code)]
#[path = "run_protocol.rs"]
mod protocol;

use bytes::{Bytes, BytesMut};
use futures::stream;
use http_body_util::{BodyExt, StreamBody};
use protocol::*;
use restate_sdk::prelude::Endpoint;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const SLEEP: u16 = 0x040c;
const SLEEP_COMPLETION: u16 = 0x800c;

#[derive(Clone, PartialEq, prost::Message)]
struct Sleep {
    #[prost(uint32, tag = "11")]
    completion_id: u32,
}

#[derive(Clone, Copy)]
pub enum Operation {
    Run,
    Timer,
}

async fn invoke(
    endpoint: &Endpoint,
    service: &str,
    operation: Operation,
    operations: usize,
    expected: u64,
) -> Duration {
    let (tx, rx) = mpsc::unbounded_channel();
    tx.send(encode(
        START,
        &Start {
            id: Bytes::from_static(b"context-benchmark"),
            debug_id: "context-benchmark".into(),
            known_entries: 1,
            retry_count: 0,
        },
    ))
    .unwrap();
    tx.send(encode(
        INPUT,
        &Input {
            value: Some(Value {
                content: Bytes::new(),
            }),
        },
    ))
    .unwrap();
    let input = StreamBody::new(stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|data| (Ok::<_, Infallible>(http_body::Frame::data(data)), rx))
    }));
    let request = http::Request::builder()
        .uri(format!("/invoke/{service}/run"))
        .header("content-type", "application/vnd.restate.invocation.v7")
        .body(input)
        .unwrap();
    let mut buffer = BytesMut::new();
    let mut frames = VecDeque::new();
    let mut commands = 0;
    let mut proposals = 0;
    let mut result = None;

    // Include SDK dispatch, execution, and the in-memory protocol round trips.
    // Runtime setup, endpoint binding, and request construction are outside timing.
    let start = Instant::now();
    let response = endpoint.handle(request);
    assert_eq!(response.status(), http::StatusCode::OK);
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.unwrap().into_data() {
            buffer.extend_from_slice(&data);
            decode(&mut buffer, &mut frames);
        }
        while let Some(frame) = frames.pop_front() {
            match frame.kind {
                RUN => commands += 1,
                PROPOSAL => {
                    proposals += 1;
                    let proposal = frame.decode::<Proposal>();
                    tx.send(if frame.requests_ack {
                        encode(
                            ACK,
                            &Ack {
                                completion_id: proposal.completion_id,
                            },
                        )
                    } else {
                        completion(&proposal)
                    })
                    .unwrap();
                }
                SLEEP => {
                    commands += 1;
                    tx.send(encode(
                        SLEEP_COMPLETION,
                        &SleepCompletion {
                            completion_id: frame.decode::<Sleep>().completion_id,
                            void: Some(Void {}),
                        },
                    ))
                    .unwrap();
                }
                OUTPUT => result = Some(frame.decode::<Output>()),
                END => {
                    let elapsed = start.elapsed();
                    assert_eq!(commands, operations);
                    assert_eq!(
                        proposals,
                        match operation {
                            Operation::Run => operations,
                            Operation::Timer => 0,
                        }
                    );
                    let result = result.expect("missing handler output");
                    assert_eq!(result.failure, None);
                    assert_eq!(
                        serde_json::from_slice::<u64>(&result.value.unwrap().content).unwrap(),
                        expected
                    );
                    return elapsed;
                }
                AWAITING => {}
                ERROR => panic!(
                    "SDK protocol error: {:?}",
                    frame.decode::<protocol::Error>()
                ),
                other => panic!("unexpected protocol message: {other:#x}"),
            }
        }
        // The in-memory peer shares Tokio's task budget with the SDK. Yield
        // between response chunks so queued output cannot starve input polling.
        tokio::task::yield_now().await;
    }
    panic!("response ended without End");
}

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .map(|value| value.parse().expect("invalid benchmark setting"))
        .unwrap_or(default)
}

pub async fn measure(
    label: &str,
    endpoint: Endpoint,
    service: &str,
    operation: Operation,
    operations: usize,
    expected: u64,
) {
    let samples = setting("SDK_BENCH_SAMPLES", 10);
    let warmup = setting("SDK_BENCH_WARMUP", 3);
    let invocations = setting("SDK_BENCH_OPERATIONS_PER_SAMPLE", 1000)
        .div_ceil(operations)
        .max(1);
    for batch in 0..warmup + samples {
        let mut elapsed = Duration::ZERO;
        for _ in 0..invocations {
            elapsed += tokio::time::timeout(
                Duration::from_secs(30),
                invoke(&endpoint, service, operation, operations, expected),
            )
            .await
            .expect("benchmark invocation stalled");
        }
        if batch >= warmup {
            println!(
                "SDK_BENCH workload={label} operations={operations} invocations={invocations} sample={} elapsed_ns={} ns_per_invocation={:.3} ns_per_operation={:.3}",
                batch - warmup,
                elapsed.as_nanos(),
                elapsed.as_nanos() as f64 / invocations as f64,
                elapsed.as_nanos() as f64 / (invocations * operations) as f64,
            );
        }
    }
}
