use anyhow::anyhow;
use futures::TryFutureExt;
use restate_sdk::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
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
    #[serde(rename = "sleep")]
    Sleep { timeout_millis: u64 },
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

#[restate_sdk::object]
#[name = "VirtualObjectCommandInterpreter"]
pub(crate) trait VirtualObjectCommandInterpreter {
    #[name = "interpretCommands"]
    async fn interpret_commands(req: Json<InterpretRequest>) -> HandlerResult<String>;

    #[name = "resolveAwakeable"]
    #[shared]
    async fn resolve_awakeable(req: Json<ResolveAwakeable>) -> HandlerResult<()>;

    #[name = "rejectAwakeable"]
    #[shared]
    async fn reject_awakeable(req: Json<RejectAwakeable>) -> HandlerResult<()>;

    #[name = "hasAwakeable"]
    #[shared]
    async fn has_awakeable(awakeable_key: String) -> HandlerResult<bool>;

    #[name = "getResults"]
    #[shared]
    async fn get_results() -> HandlerResult<Json<Vec<String>>>;
}

pub(crate) struct VirtualObjectCommandInterpreterImpl;

impl VirtualObjectCommandInterpreter for VirtualObjectCommandInterpreterImpl {
    async fn interpret_commands(
        &self,
        context: ObjectContext<'_>,
        Json(req): Json<InterpretRequest>,
    ) -> HandlerResult<String> {
        let mut last_result: String = Default::default();

        for cmd in req.commands {
            match cmd {
                Command::AwaitAny { .. } => {
                    Err(anyhow!("AwaitAny is currently unsupported in the Rust SDK"))?
                }
                Command::AwaitAnySuccessful { .. } => Err(anyhow!(
                    "AwaitAnySuccessful is currently unsupported in the Rust SDK"
                ))?,
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
                        AwaitableCommand::Sleep { timeout_millis } => {
                            context
                                .sleep(Duration::from_millis(timeout_millis))
                                .map_ok(|_| "sleep".to_string())
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
