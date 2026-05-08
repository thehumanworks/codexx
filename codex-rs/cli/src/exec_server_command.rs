use clap::Parser;
use codex_arg0::Arg0DispatchPaths;

#[derive(Debug, Parser)]
pub struct ExecServerCommand {
    /// Transport endpoint URL. Supported values: `ws://IP:PORT` (default), `stdio`, `stdio://`.
    #[arg(long = "listen", value_name = "URL", conflicts_with = "remote")]
    listen: Option<String>,

    /// Register this exec-server as a remote executor using the given base URL.
    #[arg(long = "remote", value_name = "URL", requires = "executor_id")]
    remote: Option<String>,

    /// Executor id to attach to when registering remotely.
    #[arg(long = "executor-id", value_name = "ID")]
    executor_id: Option<String>,

    /// Human-readable executor name.
    #[arg(long = "name", value_name = "NAME")]
    name: Option<String>,
}

pub async fn run_exec_server_command(
    cmd: ExecServerCommand,
    arg0_paths: &Arg0DispatchPaths,
) -> anyhow::Result<()> {
    let codex_self_exe = arg0_paths
        .codex_self_exe
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Codex executable path is not configured"))?;
    let runtime_paths = codex_exec_server::ExecServerRuntimePaths::new(
        codex_self_exe,
        arg0_paths.codex_linux_sandbox_exe.clone(),
    )?;
    if let Some(base_url) = cmd.remote {
        let executor_id = cmd
            .executor_id
            .ok_or_else(|| anyhow::anyhow!("--executor-id is required when --remote is set"))?;
        let mut remote_config =
            codex_exec_server::RemoteExecutorConfig::new(base_url, executor_id)?;
        if let Some(name) = cmd.name {
            remote_config.name = name;
        }
        codex_exec_server::run_remote_executor(remote_config, runtime_paths).await?;
        return Ok(());
    }
    let listen_url = cmd
        .listen
        .as_deref()
        .unwrap_or(codex_exec_server::DEFAULT_LISTEN_URL);
    codex_exec_server::run_main(listen_url, runtime_paths)
        .await
        .map_err(anyhow::Error::from_boxed)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parses_default_listen_mode() {
        let command = ExecServerCommand::try_parse_from(["codex-exec-server"]).unwrap();

        assert_eq!(command.listen, None);
        assert_eq!(command.remote, None);
        assert_eq!(command.executor_id, None);
        assert_eq!(command.name, None);
    }

    #[test]
    fn parses_explicit_listen_mode() {
        let command =
            ExecServerCommand::try_parse_from(["codex-exec-server", "--listen", "stdio"]).unwrap();

        assert_eq!(command.listen.as_deref(), Some("stdio"));
        assert_eq!(command.remote, None);
        assert_eq!(command.executor_id, None);
        assert_eq!(command.name, None);
    }

    #[test]
    fn parses_remote_registration_mode() {
        let command = ExecServerCommand::try_parse_from([
            "codex-exec-server",
            "--remote",
            "https://example.test",
            "--executor-id",
            "executor-1",
            "--name",
            "worker",
        ])
        .unwrap();

        assert_eq!(command.listen, None);
        assert_eq!(command.remote.as_deref(), Some("https://example.test"));
        assert_eq!(command.executor_id.as_deref(), Some("executor-1"));
        assert_eq!(command.name.as_deref(), Some("worker"));
    }

    #[test]
    fn rejects_remote_without_executor_id() {
        let error = ExecServerCommand::try_parse_from([
            "codex-exec-server",
            "--remote",
            "https://example.test",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn rejects_listen_with_remote() {
        let error = ExecServerCommand::try_parse_from([
            "codex-exec-server",
            "--listen",
            "stdio",
            "--remote",
            "https://example.test",
            "--executor-id",
            "executor-1",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }
}
