use restate_sdk::context::DurableFuture;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::time::Duration;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InterpretRequest {
    commands: Vec<Command>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
#[serde(rename_all_fields = "camelCase")]
pub(crate) enum Command {
    #[serde(rename = "awaitAnySuccessful")]
    AwaitAnySuccessful { commands: Vec<AwaitableCommand> },
    #[serde(rename = "awaitAny")]
    AwaitAny { commands: Vec<AwaitableCommand> },
    #[serde(rename = "awaitOne")]
    AwaitOne { command: AwaitableCommand },
    #[serde(rename = "awaitAwakeableOrTimeout")]
    AwaitAwakeableOrTimeout {
        awakeable_key: String,
        timeout_millis: u64,
    },
    #[serde(rename = "awaitFirstSucceededOrAllFailed")]
    AwaitFirstSucceededOrAllFailed { commands: Vec<AwaitableCommand> },
    #[serde(rename = "awaitFirstCompleted")]
    AwaitFirstCompleted { commands: Vec<AwaitableCommand> },
    #[serde(rename = "awaitAllSucceededOrFirstFailed")]
    AwaitAllSucceededOrFirstFailed { commands: Vec<AwaitableCommand> },
    #[serde(rename = "awaitAllCompleted")]
    AwaitAllCompleted { commands: Vec<AwaitableCommand> },
    #[serde(rename = "resolveAwakeable")]
    ResolveAwakeable {
        awakeable_key: String,
        value: String,
    },
    #[serde(rename = "rejectAwakeable")]
    RejectAwakeable {
        awakeable_key: String,
        reason: String,
    },
    #[serde(rename = "getEnvVariable")]
    GetEnvVariable { env_name: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
#[serde(rename_all_fields = "camelCase")]
pub(crate) enum AwaitableCommand {
    #[serde(rename = "createAwakeable")]
    CreateAwakeable { awakeable_key: String },
    #[serde(rename = "createSignal")]
    CreateSignal { signal_name: String },
    #[serde(rename = "sleep")]
    Sleep { timeout_millis: u64 },
    #[serde(rename = "runReturns")]
    RunReturns { value: String },
    #[serde(rename = "runThrowTerminalException")]
    RunThrowTerminalException { reason: String },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolveAwakeable {
    awakeable_key: String,
    value: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RejectAwakeable {
    awakeable_key: String,
    reason: String,
}

/// The combinator semantics applied over a list of [`AwaitableCommand`]s.
#[derive(Clone, Copy)]
enum Combinator {
    /// `Promise.race`: settle with whatever the first command to complete does.
    FirstCompleted,
    /// `Promise.any`: return the first successful value, or throw the last error if all fail.
    FirstSucceeded,
    /// `Promise.all`: pipe-join all values in input order, or throw on the first failure.
    AllSucceeded,
    /// `Promise.allSettled`: pipe-join `ok:<value>` / `err:<reason>` in input order, never throws.
    AllCompleted,
}

type DurableFutureString<'ctx> =
    Pin<Box<dyn DurableFuture<Output = Result<String, TerminalError>> + Send + 'ctx>>;

/// Turn an [`AwaitableCommand`] into a boxed durable future resolving to a `String`, registering
/// awakeable state as needed. Sleep is normalized to the `"sleep"` value via
/// [`DurableFuture::map_ok`] so all awaitables share one output type.
///
/// `run` commands are a `RunFuture`, not a [`DurableFuture`], so they cannot participate in a
/// combinator: this errors out for them.
fn build_awaitable<'ctx>(
    context: &ObjectContext<'ctx>,
    command: AwaitableCommand,
) -> Result<DurableFutureString<'ctx>, TerminalError> {
    Ok(match command {
        AwaitableCommand::CreateAwakeable { awakeable_key } => {
            let (awakeable_id, fut) = context.awakeable::<String>();
            context.set::<String>(&format!("awk-{awakeable_key}"), awakeable_id);
            Box::pin(fut)
        }
        AwaitableCommand::CreateSignal { signal_name } => {
            Box::pin(context.signal::<String>(&signal_name))
        }
        AwaitableCommand::Sleep { timeout_millis } => Box::pin(
            context
                .sleep(Duration::from_millis(timeout_millis))
                .map_ok(|()| "sleep".to_string()),
        ),
        AwaitableCommand::RunReturns { .. }
        | AwaitableCommand::RunThrowTerminalException { .. } => {
            return Err(TerminalError::new(
                "run commands cannot participate in a combinator in the Rust SDK",
            ));
        }
    })
}

/// Drive a set of awaitables through the given combinator semantics using
/// [`DurableFuturesUnordered`], which yields each result paired with its stable input index as the
/// futures complete.
async fn run_combinator(
    awaitables: Vec<DurableFutureString<'_>>,
    combinator: Combinator,
) -> Result<String, TerminalError> {
    let n = awaitables.len();
    let mut futures: DurableFuturesUnordered<_> = awaitables.into_iter().collect();
    let mut results: Vec<Option<Result<String, TerminalError>>> =
        std::iter::repeat_with(|| None).take(n).collect();
    let mut last_error: Option<TerminalError> = None;

    while let Some((idx, result)) = futures.next().await? {
        match combinator {
            Combinator::FirstCompleted => return result,
            Combinator::FirstSucceeded => match result {
                Ok(value) => return Ok(value),
                Err(e) => last_error = Some(e),
            },
            Combinator::AllSucceeded => match result {
                Ok(value) => results[idx] = Some(Ok(value)),
                Err(e) => return Err(e),
            },
            Combinator::AllCompleted => results[idx] = Some(result),
        }
    }

    Ok(match combinator {
        // With no command completing successfully we surface the last failure (or empty list).
        Combinator::FirstCompleted => String::default(),
        Combinator::FirstSucceeded => {
            return Err(
                last_error.unwrap_or_else(|| TerminalError::new("no awaitable commands to await"))
            );
        }
        Combinator::AllSucceeded => results
            .into_iter()
            .map(|r| {
                r.expect("all commands completed")
                    .expect("all commands succeeded")
            })
            .collect::<Vec<_>>()
            .join("|"),
        Combinator::AllCompleted => results
            .into_iter()
            .map(|r| match r.expect("all commands completed") {
                Ok(value) => format!("ok:{value}"),
                Err(e) => format!("err:{}", e.message()),
            })
            .collect::<Vec<_>>()
            .join("|"),
    })
}

pub(crate) struct VirtualObjectCommandInterpreter;

#[object(name = "VirtualObjectCommandInterpreter")]
impl VirtualObjectCommandInterpreter {
    #[handler(name = "interpretCommands")]
    async fn interpret_commands(
        &self,
        context: ObjectContext<'_>,
        req: Json<InterpretRequest>,
    ) -> HandlerResult<String> {
        let Json(req) = req;
        let mut last_result: String = Default::default();

        for cmd in req.commands {
            match cmd {
                Command::AwaitAny { commands } | Command::AwaitFirstCompleted { commands } => {
                    last_result = run_combinator(
                        build_awaitables(&context, commands)?,
                        Combinator::FirstCompleted,
                    )
                    .await?;
                }
                Command::AwaitAnySuccessful { commands }
                | Command::AwaitFirstSucceededOrAllFailed { commands } => {
                    last_result = run_combinator(
                        build_awaitables(&context, commands)?,
                        Combinator::FirstSucceeded,
                    )
                    .await?;
                }
                Command::AwaitAllSucceededOrFirstFailed { commands } => {
                    last_result = run_combinator(
                        build_awaitables(&context, commands)?,
                        Combinator::AllSucceeded,
                    )
                    .await?;
                }
                Command::AwaitAllCompleted { commands } => {
                    last_result = run_combinator(
                        build_awaitables(&context, commands)?,
                        Combinator::AllCompleted,
                    )
                    .await?;
                }
                Command::AwaitAwakeableOrTimeout {
                    awakeable_key,
                    timeout_millis,
                } => {
                    let (awakeable_id, awk_fut) = context.awakeable::<String>();
                    context.set::<String>(&format!("awk-{awakeable_key}"), awakeable_id);

                    last_result = restate_sdk::select! {
                        res = awk_fut => {
                            res
                        },
                        _ = context.sleep(Duration::from_millis(timeout_millis)) => {
                            Err(TerminalError::new("await-timeout"))
                        }
                    }?;
                }
                Command::AwaitOne { command } => {
                    last_result = match command {
                        AwaitableCommand::CreateAwakeable { awakeable_key } => {
                            let (awakeable_id, fut) = context.awakeable::<String>();
                            context.set::<String>(&format!("awk-{awakeable_key}"), awakeable_id);
                            fut.await?
                        }
                        AwaitableCommand::CreateSignal { signal_name } => {
                            context.signal::<String>(&signal_name).await?
                        }
                        AwaitableCommand::Sleep { timeout_millis } => {
                            context
                                .sleep(Duration::from_millis(timeout_millis))
                                .map_ok(|_| "sleep".to_string())
                                .await?
                        }
                        AwaitableCommand::RunReturns { value } => {
                            context
                                .run::<_, _, String>(|| async move { Ok(value) })
                                .await?
                        }
                        AwaitableCommand::RunThrowTerminalException { reason } => {
                            context
                                .run::<_, _, String>(
                                    || async move { Err(TerminalError::new(reason))? },
                                )
                                .await?
                        }
                    }
                }
                Command::GetEnvVariable { env_name } => {
                    last_result = std::env::var(env_name).ok().unwrap_or_default();
                }
                Command::ResolveAwakeable {
                    awakeable_key,
                    value,
                } => {
                    let Some(awakeable_id) = context
                        .get::<String>(&format!("awk-{awakeable_key}"))
                        .await?
                    else {
                        Err(TerminalError::new(
                            "Awakeable is not registered yet".to_string(),
                        ))?
                    };

                    context.resolve_awakeable(&awakeable_id, value);
                    last_result = Default::default();
                }
                Command::RejectAwakeable {
                    awakeable_key,
                    reason,
                } => {
                    let Some(awakeable_id) = context
                        .get::<String>(&format!("awk-{awakeable_key}"))
                        .await?
                    else {
                        Err(TerminalError::new(
                            "Awakeable is not registered yet".to_string(),
                        ))?
                    };

                    context.reject_awakeable(&awakeable_id, TerminalError::new(reason));
                    last_result = Default::default();
                }
            }

            let mut old_results = context
                .get::<Json<Vec<String>>>("results")
                .await?
                .unwrap_or_default()
                .into_inner();
            old_results.push(last_result.clone());
            context.set("results", Json(old_results));
        }

        Ok(last_result)
    }

    #[handler(name = "resolveAwakeable")]
    async fn resolve_awakeable(
        &self,
        context: SharedObjectContext<'_>,
        req: Json<ResolveAwakeable>,
    ) -> Result<(), HandlerError> {
        let ResolveAwakeable {
            awakeable_key,
            value,
        } = req.into_inner();
        let Some(awakeable_id) = context
            .get::<String>(&format!("awk-{awakeable_key}"))
            .await?
        else {
            Err(TerminalError::new(
                "Awakeable is not registered yet".to_string(),
            ))?
        };

        context.resolve_awakeable(&awakeable_id, value);

        Ok(())
    }

    #[handler(name = "rejectAwakeable")]
    async fn reject_awakeable(
        &self,
        context: SharedObjectContext<'_>,
        req: Json<RejectAwakeable>,
    ) -> Result<(), HandlerError> {
        let RejectAwakeable {
            awakeable_key,
            reason,
        } = req.into_inner();
        let Some(awakeable_id) = context
            .get::<String>(&format!("awk-{awakeable_key}"))
            .await?
        else {
            Err(TerminalError::new(
                "Awakeable is not registered yet".to_string(),
            ))?
        };

        context.reject_awakeable(&awakeable_id, TerminalError::new(reason));

        Ok(())
    }

    #[handler(name = "hasAwakeable")]
    async fn has_awakeable(
        &self,
        context: SharedObjectContext<'_>,
        awakeable_key: String,
    ) -> Result<bool, HandlerError> {
        Ok(context
            .get::<String>(&format!("awk-{awakeable_key}"))
            .await?
            .is_some())
    }

    #[handler(name = "getResults")]
    async fn get_results(
        &self,
        context: SharedObjectContext<'_>,
    ) -> Result<Json<Vec<String>>, HandlerError> {
        Ok(context
            .get::<Json<Vec<String>>>("results")
            .await?
            .unwrap_or_default())
    }
}

/// Build a boxed durable future per command (see [`build_awaitable`]), erroring if any is a `run`
/// (which cannot be combined).
fn build_awaitables<'ctx>(
    context: &ObjectContext<'ctx>,
    commands: Vec<AwaitableCommand>,
) -> Result<Vec<DurableFutureString<'ctx>>, TerminalError> {
    commands
        .into_iter()
        .map(|command| build_awaitable(context, command))
        .collect()
}
