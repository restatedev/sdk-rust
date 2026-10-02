#[path = "support/run_protocol.rs"]
mod protocol;

use bytes::{Bytes, BytesMut};
use futures::stream;
use http_body::Body;
use http_body_util::{BodyExt, StreamBody};
use protocol::*;
use restate_sdk::context::DurableFuture;
use restate_sdk::endpoint::ResponseBody;
use restate_sdk::prelude::*;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::future::Future;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

const TIMEOUT: Duration = Duration::from_secs(3);

struct LegacyRuns(Arc<[AtomicUsize; 3]>);
#[service]
impl LegacyRuns {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<Json<Vec<u32>>> {
        let runs = (0..3).map(|i| {
            let counts = self.0.clone();
            ctx.run(move || async move {
                counts[i].fetch_add(1, Ordering::SeqCst);
                Ok(i as u32)
            })
            .name(format!("run-{i}"))
        });
        Ok(Json(futures::future::try_join_all(runs).await?))
    }
}

struct Invocation {
    input: Option<mpsc::UnboundedSender<Bytes>>,
    body: ResponseBody,
    buffer: BytesMut,
    frames: VecDeque<protocol::Frame>,
}
impl Invocation {
    fn new(
        endpoint: &Endpoint,
        service: &str,
        version: u32,
        journal: &[Bytes],
        retry_count: u32,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<Bytes>();
        tx.send(encode(
            START,
            &Start {
                id: Bytes::from_static(b"concurrent-runs"),
                debug_id: "concurrent-runs".to_owned(),
                known_entries: journal.len() as u32,
                retry_count,
            },
        ))
        .unwrap();
        for entry in journal {
            tx.send(entry.clone()).unwrap();
        }
        let body = StreamBody::new(stream::unfold(rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|data| (Ok::<_, Infallible>(http_body::Frame::data(data)), rx))
        }));
        let request = http::Request::builder()
            .uri(format!("/invoke/{service}/run"))
            .header(
                "content-type",
                format!("application/vnd.restate.invocation.v{version}"),
            )
            .body(body)
            .unwrap();
        let response = endpoint.handle(request);
        assert_eq!(response.status(), http::StatusCode::OK);
        Self {
            input: Some(tx),
            body: response.into_body(),
            buffer: BytesMut::new(),
            frames: VecDeque::new(),
        }
    }
    fn send(&self, bytes: Bytes) {
        self.input.as_ref().unwrap().send(bytes).unwrap();
    }
    fn close_input(&mut self) {
        self.input.take();
    }
    async fn next(&mut self) -> Option<protocol::Frame> {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                if let Some(frame) = self.frames.pop_front() {
                    return Some(frame);
                }
                let frame = self.body.frame().await?.unwrap();
                if let Ok(data) = frame.into_data() {
                    self.buffer.extend_from_slice(&data);
                    decode(&mut self.buffer, &mut self.frames);
                }
            }
        })
        .await
        .expect("SDK made no progress within test deadline")
    }
    async fn drive_until(&mut self, predicate: impl Fn() -> bool) {
        tokio::time::timeout(
            TIMEOUT,
            poll_fn(|cx| {
                loop {
                    match Pin::new(&mut self.body).poll_frame(cx) {
                        Poll::Ready(Some(Ok(frame))) => {
                            if let Ok(data) = frame.into_data() {
                                self.buffer.extend_from_slice(&data);
                                decode(&mut self.buffer, &mut self.frames);
                            }
                        }
                        Poll::Ready(None) => panic!("response ended before predicate"),
                        Poll::Pending => break,
                        Poll::Ready(Some(Err(e))) => panic!("response body error: {e}"),
                    }
                }
                if let Some(frame) = self.frames.iter().find(|frame| frame.kind == ERROR) {
                    panic!(
                        "SDK protocol error: {:?}",
                        frame.decode::<protocol::Error>()
                    );
                }
                if predicate() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }),
        )
        .await
        .expect("predicate was never reached");
    }
    async fn assert_pending(&mut self) {
        poll_fn(|cx| {
            loop {
                match Pin::new(&mut self.body).poll_frame(cx) {
                    Poll::Pending => break,
                    Poll::Ready(Some(Ok(frame))) => {
                        if let Ok(data) = frame.into_data() {
                            self.buffer.extend_from_slice(&data);
                            decode(&mut self.buffer, &mut self.frames);
                        }
                    }
                    other => panic!("expected parked response, got {other:?}"),
                }
            }
            assert!(
                self.frames
                    .iter()
                    .all(|frame| matches!(frame.kind, RUN | AWAITING)),
                "SDK resolved or stopped before the runtime acknowledged a result"
            );
            Poll::Ready(())
        })
        .await;
    }
    fn acknowledge(&self, frame: &protocol::Frame) {
        let proposal = frame.decode::<Proposal>();
        self.send(if frame.requests_ack {
            encode(
                ACK,
                &Ack {
                    completion_id: proposal.completion_id,
                },
            )
        } else {
            completion(&proposal)
        });
    }
    async fn through(&mut self, kind: u16) -> protocol::Frame {
        loop {
            let frame = self
                .next()
                .await
                .expect("response ended before expected message");
            assert_ne!(
                frame.kind,
                ERROR,
                "SDK error: {:?}",
                frame.decode::<protocol::Error>()
            );
            assert_ne!(frame.kind, SUSPENSION, "unexpected suspension");
            if frame.kind == kind {
                return frame;
            }
        }
    }
}
fn input() -> Bytes {
    encode(
        INPUT,
        &Input {
            value: Some(Value {
                content: Bytes::new(),
            }),
        },
    )
}
fn partial_journal() -> Vec<Bytes> {
    let mut journal = vec![input()];
    journal.extend((0..3).map(|i| {
        encode(
            RUN,
            &protocol::Run {
                completion_id: i + 1,
                name: format!("run-{i}"),
            },
        )
    }));
    journal.push(encode(
        COMPLETION,
        &Completion {
            completion_id: 3,
            value: Some(Value {
                content: Bytes::from_static(b"2"),
            }),
            failure: None,
        },
    ));
    journal
}

#[tokio::test]
async fn legacy_partial_replay_reproduces_journal_mismatch() {
    for version in [6, 7] {
        let counts = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let endpoint = Endpoint::builder().bind(LegacyRuns(counts.clone())).build();
        let mut invocation =
            Invocation::new(&endpoint, "LegacyRuns", version, &partial_journal(), 0);
        invocation.close_input();
        let error = loop {
            let frame = invocation.next().await.expect("expected journal mismatch");
            if frame.kind == ERROR {
                break frame.decode::<protocol::Error>();
            }
        };
        assert_eq!(error.code, 570);
        assert!(counts.iter().all(|count| count.load(Ordering::SeqCst) == 0));
    }
}

struct InvalidStartedRun {
    starts: Arc<AtomicUsize>,
    select: bool,
}

#[service]
impl InvalidStartedRun {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<u32> {
        let starts = self.starts.clone();
        let run = ctx
            .run(move || async move {
                starts.fetch_add(1, Ordering::SeqCst);
                Ok(42u32)
            })
            .name("different-from-journal")
            .start();
        assert!(restate_sdk::context::macro_support::SealedDurableFuture::handle(&run).is_none());
        if self.select {
            let run: Pin<Box<dyn DurableFuture<Output = Result<u32, TerminalError>> + Send>> =
                Box::pin(
                    run.map_ok(|value| value + 1).map_err(|error| {
                        TerminalError::new_with_code(error.code(), "mapped error")
                    }),
                );
            let mut runs = DurableFuturesUnordered::new();
            runs.push(run);
            runs.next().await?;
            unreachable!("Failed registration must trap durable selection");
        }
        Ok(run.await?)
    }
}

#[tokio::test]
async fn failed_started_registration_traps_await_and_selection_without_a_handle() {
    for version in [6, 7] {
        for select in [false, true] {
            let starts = Arc::new(AtomicUsize::new(0));
            let endpoint = Endpoint::builder()
                .bind(InvalidStartedRun {
                    starts: starts.clone(),
                    select,
                })
                .build();
            let mut invocation = Invocation::new(
                &endpoint,
                "InvalidStartedRun",
                version,
                &partial_journal(),
                0,
            );
            invocation.close_input();
            let mut errors = Vec::new();
            while let Some(frame) = invocation.next().await {
                match frame.kind {
                    ERROR => errors.push(frame.decode::<protocol::Error>()),
                    PROPOSAL | OUTPUT => panic!("Failed registration made progress"),
                    _ => {}
                }
            }
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].code, 570);
            assert_eq!(starts.load(Ordering::SeqCst), 0);
        }
    }
}

struct PolledRun;

#[service]
impl PolledRun {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<u32> {
        let mut run = ctx.run(|| std::future::ready(Ok(42u32)));
        poll_fn(|cx| {
            assert!(Pin::new(&mut run).poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.start()))
            .err()
            .expect("Starting an already-polled Unpin run must panic");
        assert_eq!(
            panic.downcast_ref::<&str>().copied(),
            Some("An action cannot be started after it has been polled")
        );
        Ok(42)
    }
}

#[tokio::test]
async fn starting_an_already_polled_unpin_run_panics() {
    for version in [6, 7] {
        let endpoint = Endpoint::builder().bind(PolledRun).build();
        let mut invocation = Invocation::new(&endpoint, "PolledRun", version, &[input()], 0);
        let output = finish(&mut invocation).await;
        assert_eq!(output.value.unwrap().content, Bytes::from_static(b"42"));
    }
}

#[derive(Default)]
struct RunStats {
    starts: [AtomicUsize; 3],
    drops: [AtomicUsize; 3],
    gates: [Notify; 3],
}
impl RunStats {
    fn counts(&self) -> [usize; 3] {
        std::array::from_fn(|i| self.starts[i].load(Ordering::SeqCst))
    }
    fn drops(&self) -> [usize; 3] {
        std::array::from_fn(|i| self.drops[i].load(Ordering::SeqCst))
    }
}
struct DropRun(Arc<RunStats>, usize);
impl Drop for DropRun {
    fn drop(&mut self) {
        self.0.drops[self.1].fetch_add(1, Ordering::SeqCst);
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Join,
    Unordered,
    DropFirst,
    Borrowed,
    Mixed,
    CancelSettled,
    CancelCleanup,
    CancelSelect,
    CancelTimer,
    CancelRead,
    CancelSelectReady,
    Sleep,
    Select,
    Retry,
    Terminal,
    BoundedRetry,
}
struct StartedRuns {
    stats: Arc<RunStats>,
    mode: Mode,
}
#[service]
impl StartedRuns {
    #[handler]
    async fn run(&self, ctx: Context<'_>) -> HandlerResult<Json<Vec<u32>>> {
        let mode = self.mode;
        if matches!(mode, Mode::Borrowed) {
            let value = 42;
            return Ok(Json(vec![ctx.run(|| async { Ok(value) }).await?]));
        }
        let make = |i: usize| {
            let stats = self.stats.clone();
            ctx.run(move || async move {
                stats.starts[i].fetch_add(1, Ordering::SeqCst);
                let _guard = DropRun(stats.clone(), i);
                stats.gates[i].notified().await;
                if i == 0 && matches!(mode, Mode::Retry | Mode::BoundedRetry) {
                    return Err(std::io::Error::other("retry run-0").into());
                }
                if i == 0 && matches!(mode, Mode::Terminal) {
                    return Err(TerminalError::new_with_code(418, "terminal run-0").into());
                }
                Ok(i as u32)
            })
            .name(format!("run-{i}"))
        };
        match mode {
            Mode::Join | Mode::Retry | Mode::Terminal | Mode::BoundedRetry => {
                let futures = (0..3)
                    .map(|i| {
                        make(i)
                            .retry_policy(
                                RunRetryPolicy::new()
                                    .initial_delay(Duration::from_millis(25))
                                    .max_attempts(if matches!(mode, Mode::BoundedRetry) {
                                        1
                                    } else {
                                        5
                                    }),
                            )
                            .start()
                    })
                    .collect::<Vec<_>>();
                Ok(Json(futures::future::try_join_all(futures).await?))
            }
            Mode::CancelTimer => {
                let timer = ctx.sleep(Duration::from_secs(10));
                let run = make(1).start();
                let run_error = run.await.unwrap_err().code();
                let mut old_results = DurableFuturesUnordered::new();
                old_results.push(timer);
                let selection_error = old_results.next().await.unwrap_err().code();
                Ok(Json(vec![run_error as u32, selection_error as u32]))
            }
            Mode::CancelRead => {
                let first = make(0).start();
                let second = make(1).start();
                let second_error = second.await.unwrap_err().code();
                Ok(Json(vec![second_error as u32, first.await?]))
            }
            Mode::CancelSelectReady => {
                let first = make(0).start();
                let second = make(1).start();
                let second_error = second.await.unwrap_err().code();
                let mut old_results = DurableFuturesUnordered::new();
                old_results.push(first);
                let (index, value) = old_results.next().await?.unwrap();
                Ok(Json(vec![second_error as u32, index as u32, value?]))
            }
            Mode::CancelSelect => {
                let first = make(0).start();
                let second = make(1).start();
                let second_error = second.await.unwrap_err().code();
                let mut old_results = DurableFuturesUnordered::new();
                old_results.push(first);
                let selection_error = old_results.next().await.unwrap_err().code();
                Ok(Json(vec![second_error as u32, selection_error as u32]))
            }
            Mode::CancelSettled | Mode::CancelCleanup => {
                let first = make(0).start();
                let second = make(1).start();
                let third = make(2).start();
                let results = futures::join!(first, second, third);
                let codes = vec![
                    results.0.unwrap_err().code() as u32,
                    results.1.unwrap_err().code() as u32,
                    results.2.unwrap_err().code() as u32,
                ];
                if matches!(mode, Mode::CancelCleanup) {
                    let value = ctx
                        .run(|| async { Ok(42u32) })
                        .name("cleanup")
                        .start()
                        .await?;
                    return Ok(Json(vec![value]));
                }
                Ok(Json(codes))
            }
            Mode::Mixed => {
                let stats = self.stats.clone();
                let owned = ctx
                    .run(move || async move {
                        stats.starts[0].fetch_add(1, Ordering::SeqCst);
                        stats.gates[1].notify_one();
                        Ok(0u32)
                    })
                    .name("run-0")
                    .start();
                let borrowed = &self.stats;
                let second = ctx
                    .run(|| async {
                        borrowed.starts[1].fetch_add(1, Ordering::SeqCst);
                        borrowed.gates[1].notified().await;
                        Ok(1u32)
                    })
                    .name("run-1")
                    .await?;
                Ok(Json(vec![owned.await?, second]))
            }
            Mode::Unordered => {
                use futures::StreamExt;
                let mut runs = (0..3)
                    .map(|i| make(i).start())
                    .collect::<futures::stream::FuturesUnordered<_>>();
                let mut values = Vec::new();
                while let Some(value) = runs.next().await {
                    values.push(value?);
                }
                values.sort();
                Ok(Json(values))
            }
            Mode::DropFirst => {
                let first = make(0).start();
                let second = make(1).start();
                drop(first);
                Ok(Json(vec![second.await?]))
            }
            Mode::Sleep => {
                let run = make(0).start();
                let timer = ctx.sleep(Duration::from_secs(10));
                timer.await?;
                Ok(Json(vec![run.await?]))
            }
            Mode::Select => {
                let first = make(0).start();
                let second = make(1).start();
                let mut runs = DurableFuturesUnordered::new();
                runs.push(first);
                runs.push(second);
                let (index, value) = runs.next().await?.unwrap();
                Ok(Json(vec![index as u32, value?]))
            }
            Mode::Borrowed => unreachable!(),
        }
    }
}
fn started(mode: Mode) -> (Endpoint, Arc<RunStats>) {
    let stats = Arc::new(RunStats::default());
    (
        Endpoint::builder()
            .bind(StartedRuns {
                stats: stats.clone(),
                mode,
            })
            .build(),
        stats,
    )
}
async fn finish(invocation: &mut Invocation) -> Output {
    let output = invocation.through(OUTPUT).await.decode();
    invocation.close_input();
    assert_eq!(invocation.through(END).await.kind, END);
    assert!(invocation.next().await.is_none());
    output
}
fn values(output: Output) -> Vec<u32> {
    assert_eq!(output.failure, None);
    serde_json::from_slice(&output.value.unwrap().content).unwrap()
}

#[tokio::test]
async fn eager_commands_and_shuffled_completions() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        for i in 0..3 {
            let run = invocation.through(RUN).await.decode::<protocol::Run>();
            assert_eq!((run.completion_id, run.name), (i + 1, format!("run-{i}")));
        }
        for i in [2, 0, 1] {
            stats.gates[i].notify_one();
            let proposal = invocation.through(PROPOSAL).await;
            assert_eq!(proposal.decode::<Proposal>().completion_id, i as u32 + 1);
            invocation.acknowledge(&proposal);
        }
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1, 2]);
        assert_eq!(stats.counts(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn partial_replay_executes_only_unfinished_runs() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation =
            Invocation::new(&endpoint, "StartedRuns", version, &partial_journal(), 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 0]).await;
        for i in [1, 0] {
            stats.gates[i].notify_one();
            let frame = invocation.through(PROPOSAL).await;
            assert_eq!(frame.decode::<Proposal>().completion_id, i as u32 + 1);
            invocation.acknowledge(&frame);
        }
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1, 2]);
        assert_eq!(stats.counts(), [1, 1, 0]);
    }
}

#[tokio::test]
async fn complete_replay_never_invokes_closures() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut journal = partial_journal();
        for i in [0, 1] {
            journal.push(encode(
                COMPLETION,
                &Completion {
                    completion_id: i + 1,
                    value: Some(Value {
                        content: Bytes::from(i.to_string()),
                    }),
                    failure: None,
                },
            ));
        }
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &journal, 0);
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1, 2]);
        assert_eq!(stats.counts(), [0, 0, 0]);
    }
}

#[tokio::test]
async fn proposal_does_not_complete_future_until_runtime_acknowledges() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        let mut proposals = Vec::new();
        for i in [2, 0, 1] {
            stats.gates[i].notify_one();
            proposals.push(invocation.through(PROPOSAL).await);
        }
        while invocation
            .frames
            .front()
            .is_some_and(|f| f.kind == AWAITING)
        {
            invocation.frames.pop_front();
        }
        invocation.assert_pending().await;
        for proposal in &proposals {
            invocation.acknowledge(proposal);
        }
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1, 2]);
    }
}

#[tokio::test]
async fn eof_while_runs_execute_waits_for_proposals_then_suspends() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        invocation.close_input();
        invocation.assert_pending().await;
        for i in [2, 0, 1] {
            stats.gates[i].notify_one();
            let frame = invocation.through(PROPOSAL).await;
            assert_eq!(frame.decode::<Proposal>().completion_id, i as u32 + 1);
        }
        loop {
            let frame = invocation.next().await.unwrap();
            assert_ne!(frame.kind, ERROR);
            assert_ne!(frame.kind, OUTPUT);
            if frame.kind == SUSPENSION {
                break;
            }
        }
        assert!(invocation.next().await.is_none());
        assert_eq!(stats.drops(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn dropping_result_keeps_owned_run_alive_and_driving_it() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::DropFirst);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 0]).await;
        for i in [0, 1] {
            stats.gates[i].notify_one();
            let frame = invocation.through(PROPOSAL).await;
            assert_eq!(frame.decode::<Proposal>().completion_id, i as u32 + 1);
            invocation.acknowledge(&frame);
        }
        assert_eq!(values(finish(&mut invocation).await), vec![1]);
    }
}

#[tokio::test]
async fn dropping_response_cancels_all_pending_run_futures() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        drop(invocation);
        assert_eq!(stats.drops(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn cancellation_is_observed_while_all_closures_are_pending() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Join);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        invocation.send(encode(
            SIGNAL,
            &Signal {
                index: Some(1),
                failure: Some(Failure {
                    code: 409,
                    message: "cancelled".into(),
                }),
            },
        ));
        let output = finish(&mut invocation).await;
        assert_eq!(output.failure.unwrap().code, 409);
        assert_eq!(stats.drops(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn retryable_failure_preserves_command_attribution_and_retry_delay() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Retry);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        stats.gates[0].notify_one();
        invocation.close_input();
        let error = loop {
            let frame = invocation.next().await.unwrap();
            if frame.kind == ERROR {
                break frame.decode::<protocol::Error>();
            }
        };
        assert_eq!(error.code, 500);
        assert_eq!(error.command_index, Some(1));
        assert_eq!(error.next_retry_delay, Some(25));
        drop(invocation);
        assert_eq!(stats.drops(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn bounded_retry_failure_is_proposed_as_terminal() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::BoundedRetry);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        stats.gates[0].notify_one();
        let frame = invocation.through(PROPOSAL).await;
        let proposal = frame.decode::<Proposal>();
        assert_eq!(proposal.completion_id, 1);
        assert_eq!(proposal.failure.unwrap().code, 500);
        invocation.acknowledge(&frame);
        let output = finish(&mut invocation).await;
        assert_eq!(output.failure.unwrap().code, 500);
    }
}

#[tokio::test]
async fn terminal_failure_is_proposed_and_resolved_from_vm() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Terminal);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        stats.gates[0].notify_one();
        let frame = invocation.through(PROPOSAL).await;
        assert_eq!(frame.decode::<Proposal>().failure.unwrap().code, 418);
        invocation.acknowledge(&frame);
        assert_eq!(finish(&mut invocation).await.failure.unwrap().code, 418);
    }
}

#[tokio::test]
async fn started_runs_support_durable_selection() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Select);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 0]).await;
        stats.gates[1].notify_one();
        let frame = invocation.through(PROPOSAL).await;
        invocation.acknowledge(&frame);
        assert_eq!(values(finish(&mut invocation).await), vec![1, 1]);
        assert_eq!(stats.drops(), [1, 1, 0]);
    }
}

#[tokio::test]
async fn borrowing_sequential_run_remains_supported() {
    for version in [6, 7] {
        let (endpoint, _) = started(Mode::Borrowed);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        let proposal = invocation.through(PROPOSAL).await;
        invocation.acknowledge(&proposal);
        assert_eq!(values(finish(&mut invocation).await), vec![42]);
    }
}

#[tokio::test]
async fn input_consumer_wakes_sibling_result_futures() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Unordered);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        let mut proposals = Vec::new();
        for i in [2, 0, 1] {
            stats.gates[i].notify_one();
            proposals.push(invocation.through(PROPOSAL).await);
        }
        for proposal in proposals {
            invocation.acknowledge(&proposal);
        }
        // Keep input open until output; EOF must not rescue a stranded sibling.
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1, 2]);
    }
}

#[tokio::test]
async fn awaiting_timer_drives_registered_run_and_preserves_timer_completion() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::Sleep);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 0, 0]).await;
        stats.gates[0].notify_one();
        let proposal = invocation.through(PROPOSAL).await;
        invocation.acknowledge(&proposal);
        // The timer allocated completion 2 after the eagerly registered run.
        invocation.send(encode(
            0x800c,
            &SleepCompletion {
                completion_id: 2,
                void: Some(Void {}),
            },
        ));
        assert_eq!(values(finish(&mut invocation).await), vec![0]);
    }
}

#[tokio::test]
async fn cancellation_settles_every_existing_run_result() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::CancelSettled);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        invocation.send(encode(
            SIGNAL,
            &Signal {
                index: Some(1),
                failure: Some(Failure {
                    code: 409,
                    message: "cancelled".into(),
                }),
            },
        ));
        assert_eq!(values(finish(&mut invocation).await), vec![409, 409, 409]);
        assert_eq!(stats.drops(), [1, 1, 1]);
    }
}

#[tokio::test]
async fn cancellation_allows_new_cleanup_operation_after_pending_results_settle() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::CancelCleanup);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 1]).await;
        invocation.send(encode(
            SIGNAL,
            &Signal {
                index: Some(1),
                failure: Some(Failure {
                    code: 409,
                    message: "cancelled".into(),
                }),
            },
        ));
        let proposal = invocation.through(PROPOSAL).await;
        assert_eq!(proposal.decode::<Proposal>().completion_id, 4);
        invocation.acknowledge(&proposal);
        assert_eq!(values(finish(&mut invocation).await), vec![42]);
    }
}

#[tokio::test]
async fn sequential_borrowing_run_drives_earlier_started_run() {
    for version in [6, 7] {
        let (endpoint, _) = started(Mode::Mixed);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        for _ in 0..2 {
            let proposal = invocation.through(PROPOSAL).await;
            invocation.acknowledge(&proposal);
        }
        assert_eq!(values(finish(&mut invocation).await), vec![0, 1]);
    }
}

struct ProbeWake;
impl futures::task::ArcWake for ProbeWake {
    fn wake_by_ref(_: &Arc<Self>) {}
}

#[tokio::test]
async fn replacing_waiter_wakers_releases_old_executor_references() {
    let (endpoint, stats) = started(Mode::Join);
    let mut invocation = Invocation::new(&endpoint, "StartedRuns", 7, &[input()], 0);
    let mut weak = Vec::new();
    for _ in 0..32 {
        let probe = Arc::new(ProbeWake);
        weak.push(Arc::downgrade(&probe));
        let waker = futures::task::waker(probe);
        let mut cx = std::task::Context::from_waker(&waker);
        loop {
            match Pin::new(&mut invocation.body).poll_frame(&mut cx) {
                Poll::Pending => break,
                Poll::Ready(Some(Ok(_))) => {}
                other => panic!("unexpected response while runs remain pending: {other:?}"),
            }
        }
    }
    assert_eq!(stats.counts(), [1, 1, 1]);
    assert!(
        weak[..31].iter().all(|waker| waker.upgrade().is_none()),
        "completed poll registrations retained historical executor wakers"
    );
    drop(invocation);
    assert!(weak.iter().all(|waker| waker.upgrade().is_none()));
}

#[tokio::test]
async fn selecting_pre_cancellation_proposed_result_returns_cancellation() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::CancelSelect);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [1, 1, 0]).await;
        stats.gates[0].notify_one();
        let proposed = invocation.through(PROPOSAL).await.decode::<Proposal>();
        assert_eq!(proposed.completion_id, 1);
        // Its proposal exists, but the runtime has not acknowledged it.
        invocation.send(encode(
            SIGNAL,
            &Signal {
                index: Some(1),
                failure: Some(Failure {
                    code: 409,
                    message: "cancelled".into(),
                }),
            },
        ));
        assert_eq!(values(finish(&mut invocation).await), vec![409, 409]);
        assert_eq!(stats.drops(), [1, 1, 0]);
    }
}

#[tokio::test]
async fn selecting_pre_cancellation_timer_returns_cancellation() {
    for version in [6, 7] {
        let (endpoint, stats) = started(Mode::CancelTimer);
        let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
        invocation.drive_until(|| stats.counts() == [0, 1, 0]).await;
        invocation.send(encode(
            SIGNAL,
            &Signal {
                index: Some(1),
                failure: Some(Failure {
                    code: 409,
                    message: "cancelled".into(),
                }),
            },
        ));
        assert_eq!(values(finish(&mut invocation).await), vec![409, 409]);
        assert_eq!(stats.drops(), [0, 1, 0]);
    }
}

#[tokio::test]
async fn acknowledged_result_survives_sibling_cancellation() {
    for version in [6, 7] {
        for mode in [Mode::CancelRead, Mode::CancelSelectReady] {
            let (endpoint, stats) = started(mode);
            let mut invocation = Invocation::new(&endpoint, "StartedRuns", version, &[input()], 0);
            invocation.drive_until(|| stats.counts() == [1, 1, 0]).await;
            stats.gates[0].notify_one();
            let proposal = invocation.through(PROPOSAL).await;
            invocation.acknowledge(&proposal);
            // Process the acknowledgment before delivering cancellation.
            invocation.assert_pending().await;
            invocation.send(encode(
                SIGNAL,
                &Signal {
                    index: Some(1),
                    failure: Some(Failure {
                        code: 409,
                        message: "cancelled".into(),
                    }),
                },
            ));
            let expected = if matches!(mode, Mode::CancelRead) {
                vec![409, 0]
            } else {
                vec![409, 0, 0]
            };
            assert_eq!(values(finish(&mut invocation).await), expected);
            assert_eq!(stats.drops(), [1, 1, 0]);
        }
    }
}
