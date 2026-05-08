use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_cli::ExecServerCommand;
use codex_cli::run_exec_server_command;

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        run_exec_server_command(ExecServerCommand::parse(), &arg0_paths).await
    })
}
