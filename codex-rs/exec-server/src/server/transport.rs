use std::io::Result as IoResult;
use std::io::Write as _;
use std::net::SocketAddr;
use tokio::io;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::info;
use tracing::warn;

use crate::ExecServerRuntimePaths;
use crate::connection::JsonRpcConnection;
use crate::server::processor::ConnectionProcessor;

pub const DEFAULT_LISTEN_URL: &str = "ws://127.0.0.1:0";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ExecServerListenTransport {
    WebSocket(SocketAddr),
    Stdio,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ExecServerListenUrlParseError {
    UnsupportedListenUrl(String),
    InvalidWebSocketListenUrl(String),
}

impl std::fmt::Display for ExecServerListenUrlParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecServerListenUrlParseError::UnsupportedListenUrl(listen_url) => write!(
                f,
                "unsupported --listen URL `{listen_url}`; expected `ws://IP:PORT` or `stdio`"
            ),
            ExecServerListenUrlParseError::InvalidWebSocketListenUrl(listen_url) => write!(
                f,
                "invalid websocket --listen URL `{listen_url}`; expected `ws://IP:PORT`"
            ),
        }
    }
}

impl std::error::Error for ExecServerListenUrlParseError {}

pub(crate) fn parse_listen_url(
    listen_url: &str,
) -> Result<ExecServerListenTransport, ExecServerListenUrlParseError> {
    if matches!(listen_url, "stdio" | "stdio://") {
        return Ok(ExecServerListenTransport::Stdio);
    }

    if let Some(socket_addr) = listen_url.strip_prefix("ws://") {
        return socket_addr
            .parse::<SocketAddr>()
            .map(ExecServerListenTransport::WebSocket)
            .map_err(|_| {
                ExecServerListenUrlParseError::InvalidWebSocketListenUrl(listen_url.to_string())
            });
    }

    Err(ExecServerListenUrlParseError::UnsupportedListenUrl(
        listen_url.to_string(),
    ))
}

pub(crate) async fn run_transport(
    listen_url: &str,
    runtime_paths: ExecServerRuntimePaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match parse_listen_url(listen_url)? {
        ExecServerListenTransport::WebSocket(bind_address) => {
            run_websocket_listener(bind_address, runtime_paths).await
        }
        ExecServerListenTransport::Stdio => run_stdio_connection(runtime_paths).await,
    }
}

async fn run_stdio_connection(
    runtime_paths: ExecServerRuntimePaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_stdio_connection_with_io(io::stdin(), io::stdout(), runtime_paths).await
}

async fn run_stdio_connection_with_io<R, W>(
    reader: R,
    writer: W,
    runtime_paths: ExecServerRuntimePaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let processor = ConnectionProcessor::new(runtime_paths);
    tracing::info!("codex-exec-server listening on stdio");
    let shutdown_token = CancellationToken::new();
    let signal_shutdown_token = shutdown_token.clone();
    let signal_task = tokio::spawn(async move {
        match shutdown_signal().await {
            Ok(()) => {
                info!("received SIGTERM; shutting down codex-exec-server");
                signal_shutdown_token.cancel();
            }
            Err(err) => {
                warn!("failed to listen for exec-server shutdown signal: {err}");
            }
        }
    });
    processor
        .run_connection(
            JsonRpcConnection::from_stdio(reader, writer, "exec-server stdio".to_string()),
            shutdown_token,
        )
        .await;
    signal_task.abort();
    processor.shutdown().await;
    Ok(())
}

async fn run_websocket_listener(
    bind_address: SocketAddr,
    runtime_paths: ExecServerRuntimePaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind(bind_address).await?;
    let local_addr = listener.local_addr()?;
    let processor = ConnectionProcessor::new(runtime_paths);
    tracing::info!("codex-exec-server listening on ws://{local_addr}");
    println!("ws://{local_addr}");
    std::io::stdout().flush()?;

    let shutdown_token = CancellationToken::new();
    let connection_tasks = TaskTracker::new();
    let shutdown_signal = shutdown_signal();
    tokio::pin!(shutdown_signal);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted?,
            shutdown_result = &mut shutdown_signal => {
                if let Err(err) = shutdown_result {
                    warn!("failed to listen for exec-server shutdown signal: {err}");
                }
                info!("received SIGTERM; shutting down codex-exec-server");
                break;
            }
        };
        let (stream, peer_addr) = accepted;
        let processor = processor.clone();
        let connection_shutdown_token = shutdown_token.clone();
        connection_tasks.spawn(async move {
            let websocket = tokio::select! {
                websocket = accept_async(stream) => websocket,
                _ = connection_shutdown_token.cancelled() => {
                    return;
                }
            };
            match websocket {
                Ok(websocket) => {
                    processor
                        .run_connection(
                            JsonRpcConnection::from_websocket(
                                websocket,
                                format!("exec-server websocket {peer_addr}"),
                            ),
                            connection_shutdown_token,
                        )
                        .await;
                }
                Err(err) => {
                    warn!(
                        "failed to accept exec-server websocket connection from {peer_addr}: {err}"
                    );
                }
            }
        });
    }

    shutdown_token.cancel();
    connection_tasks.close();
    connection_tasks.wait().await;
    processor.shutdown().await;
    info!("codex-exec-server shutdown complete");
    Ok(())
}

async fn shutdown_signal() -> IoResult<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        use tokio::signal::unix::signal;

        let mut term = signal(SignalKind::terminate())?;
        let _ = term.recv().await;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;
