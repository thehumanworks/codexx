use std::io::Write as _;
use std::net::SocketAddr;
use tokio::io;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_tungstenite::accept_async;
use tracing::warn;

use crate::ExecServerRuntimePaths;
use crate::connection::JsonRpcConnection;
use crate::server::processor::ConnectionProcessor;

pub const DEFAULT_LISTEN_URL: &str = "ws://127.0.0.1:0";
const HTTP_REQUEST_PEEK_BYTES: usize = 64;
const HEALTH_REQUEST_LINE_PREFIX: &[u8] = b"GET /health HTTP/";
const HEALTH_REQUEST_MAX_BYTES: usize = 8 * 1024;
const HEALTH_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\n\
content-type: text/plain; charset=utf-8\r\n\
content-length: 3\r\n\
connection: close\r\n\
\r\n\
ok\n";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ExecServerListenTransport {
    WebSocket(SocketAddr),
    Stdio,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ConnectionPreflightRoute {
    Health,
    WebSocket,
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
    processor
        .run_connection(JsonRpcConnection::from_stdio(
            reader,
            writer,
            "exec-server stdio".to_string(),
        ))
        .await;
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

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        let processor = processor.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_connection(stream, peer_addr, processor).await {
                warn!("failed to serve exec-server connection from {peer_addr}: {err}");
            }
        });
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    processor: ConnectionProcessor,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request_prefix = [0; HTTP_REQUEST_PEEK_BYTES];
    let bytes_read = stream.peek(&mut request_prefix).await?;
    match connection_preflight_route(&request_prefix[..bytes_read]) {
        ConnectionPreflightRoute::Health => {
            read_health_check_request(&mut stream).await?;
            stream.write_all(HEALTH_RESPONSE).await?;
        }
        ConnectionPreflightRoute::WebSocket => match accept_async(stream).await {
            Ok(websocket) => {
                processor
                    .run_connection(JsonRpcConnection::from_websocket(
                        websocket,
                        format!("exec-server websocket {peer_addr}"),
                    ))
                    .await;
            }
            Err(err) => {
                warn!("failed to accept exec-server websocket connection from {peer_addr}: {err}");
            }
        },
    };

    Ok(())
}

fn connection_preflight_route(request_prefix: &[u8]) -> ConnectionPreflightRoute {
    if request_prefix.starts_with(HEALTH_REQUEST_LINE_PREFIX) {
        return ConnectionPreflightRoute::Health;
    }

    ConnectionPreflightRoute::WebSocket
}

async fn read_health_check_request(stream: &mut TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut buffer = [0; 512];
    loop {
        let bytes_read = stream.read(&mut buffer).await?;
        if bytes_read == 0 {
            return Ok(());
        }

        request.extend_from_slice(&buffer[..bytes_read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n")
            || request.len() >= HEALTH_REQUEST_MAX_BYTES
        {
            return Ok(());
        }
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;
