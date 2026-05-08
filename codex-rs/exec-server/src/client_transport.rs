use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tracing::debug;
use tracing::warn;

use crate::ExecServerClient;
use crate::ExecServerError;
use crate::client_api::RemoteExecServerConnectArgs;
use crate::client_api::StdioExecServerCommand;
use crate::client_api::StdioExecServerConnectArgs;
use crate::connection::JsonRpcConnection;

const ENVIRONMENT_CLIENT_NAME: &str = "codex-environment";
const ENVIRONMENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ENVIRONMENT_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CODEX_EXEC_SERVER_CONNECT_TIMEOUT_MS_ENV_VAR: &str =
    "CODEX_EXEC_SERVER_CONNECT_TIMEOUT_MS";
pub const CODEX_EXEC_SERVER_INITIALIZE_TIMEOUT_MS_ENV_VAR: &str =
    "CODEX_EXEC_SERVER_INITIALIZE_TIMEOUT_MS";

impl ExecServerClient {
    pub(crate) async fn connect_for_transport(
        transport_params: crate::client_api::ExecServerTransportParams,
    ) -> Result<Self, ExecServerError> {
        let connect_timeout = environment_connect_timeout();
        let initialize_timeout = environment_initialize_timeout();
        match transport_params {
            crate::client_api::ExecServerTransportParams::WebSocketUrl(websocket_url) => {
                Self::connect_websocket(RemoteExecServerConnectArgs {
                    websocket_url,
                    client_name: ENVIRONMENT_CLIENT_NAME.to_string(),
                    connect_timeout,
                    initialize_timeout,
                    resume_session_id: None,
                })
                .await
            }
            crate::client_api::ExecServerTransportParams::StdioCommand(command) => {
                Self::connect_stdio_command(StdioExecServerConnectArgs {
                    command,
                    client_name: ENVIRONMENT_CLIENT_NAME.to_string(),
                    initialize_timeout,
                    resume_session_id: None,
                })
                .await
            }
        }
    }

    pub async fn connect_websocket(
        args: RemoteExecServerConnectArgs,
    ) -> Result<Self, ExecServerError> {
        let websocket_url = args.websocket_url.clone();
        let connect_timeout = args.connect_timeout;
        let (stream, _) = timeout(connect_timeout, connect_async(websocket_url.as_str()))
            .await
            .map_err(|_| ExecServerError::WebSocketConnectTimeout {
                url: websocket_url.clone(),
                timeout: connect_timeout,
            })?
            .map_err(|source| ExecServerError::WebSocketConnect {
                url: websocket_url.clone(),
                source,
            })?;

        Self::connect(
            JsonRpcConnection::from_websocket(
                stream,
                format!("exec-server websocket {websocket_url}"),
            ),
            args.into(),
        )
        .await
    }

    pub(crate) async fn connect_stdio_command(
        args: StdioExecServerConnectArgs,
    ) -> Result<Self, ExecServerError> {
        let mut child = stdio_command_process(&args.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ExecServerError::Spawn)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            ExecServerError::Protocol("spawned exec-server command has no stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ExecServerError::Protocol("spawned exec-server command has no stdout".to_string())
        })?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => debug!("exec-server stdio stderr: {line}"),
                        Ok(None) => break,
                        Err(err) => {
                            warn!("failed to read exec-server stdio stderr: {err}");
                            break;
                        }
                    }
                }
            });
        }

        Self::connect(
            JsonRpcConnection::from_stdio(stdout, stdin, "exec-server stdio command".to_string())
                .with_child_process(child),
            args.into(),
        )
        .await
    }
}

fn stdio_command_process(stdio_command: &StdioExecServerCommand) -> Command {
    let mut command = Command::new(&stdio_command.program);
    command.args(&stdio_command.args);
    command.envs(&stdio_command.env);
    if let Some(cwd) = &stdio_command.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    command.process_group(0);
    command
}

fn environment_connect_timeout() -> Duration {
    timeout_from_env_var(
        CODEX_EXEC_SERVER_CONNECT_TIMEOUT_MS_ENV_VAR,
        ENVIRONMENT_CONNECT_TIMEOUT,
    )
}

fn environment_initialize_timeout() -> Duration {
    timeout_from_env_var(
        CODEX_EXEC_SERVER_INITIALIZE_TIMEOUT_MS_ENV_VAR,
        ENVIRONMENT_INITIALIZE_TIMEOUT,
    )
}

fn timeout_from_env_var(env_var: &str, default: Duration) -> Duration {
    timeout_from_env_value(std::env::var(env_var).ok().as_deref(), default)
}

fn timeout_from_env_value(value: Option<&str>, default: Duration) -> Duration {
    value
        .map(str::trim)
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_from_env_value_uses_milliseconds() {
        assert_eq!(
            timeout_from_env_value(Some("1234"), Duration::from_secs(5)),
            Duration::from_millis(1234)
        );
    }

    #[test]
    fn timeout_from_env_value_uses_default_for_missing_or_invalid_values() {
        let default = Duration::from_secs(5);

        assert_eq!(timeout_from_env_value(/*value*/ None, default), default);
        assert_eq!(timeout_from_env_value(Some(""), default), default);
        assert_eq!(
            timeout_from_env_value(Some("not-a-number"), default),
            default
        );
    }
}
