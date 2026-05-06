#![cfg(unix)]

mod common;

use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_exec_server::InitializeParams;
use codex_exec_server::InitializeResponse;
use common::exec_server::exec_server;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_server_serves_health_check_and_keeps_websocket_running() -> anyhow::Result<()> {
    let mut server = exec_server().await?;
    let socket_addr = server
        .websocket_url()
        .strip_prefix("ws://")
        .ok_or_else(|| anyhow::anyhow!("websocket URL should use ws://"))?;
    let mut stream = TcpStream::connect(socket_addr).await?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    assert_eq!(
        response,
        "HTTP/1.1 200 OK\r\n\
content-type: text/plain; charset=utf-8\r\n\
content-length: 3\r\n\
connection: close\r\n\
\r\n\
ok\n"
    );

    let initialize_id = server
        .send_request(
            "initialize",
            serde_json::to_value(InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: None,
            })?,
        )
        .await?;

    let response = server
        .wait_for_event(|event| {
            matches!(
                event,
                JSONRPCMessage::Response(JSONRPCResponse { id, .. }) if id == &initialize_id
            )
        })
        .await?;
    let JSONRPCMessage::Response(JSONRPCResponse { id, result }) = response else {
        panic!("expected initialize response after health check");
    };
    assert_eq!(id, initialize_id);
    let initialize_response: InitializeResponse = serde_json::from_value(result)?;
    Uuid::parse_str(&initialize_response.session_id)?;

    server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_server_reports_malformed_websocket_json_and_keeps_running() -> anyhow::Result<()> {
    let mut server = exec_server().await?;
    server.send_raw_text("not-json").await?;

    let response = server
        .wait_for_event(|event| matches!(event, JSONRPCMessage::Error(_)))
        .await?;
    let JSONRPCMessage::Error(JSONRPCError { id, error }) = response else {
        panic!("expected malformed-message error response");
    };
    assert_eq!(id, codex_app_server_protocol::RequestId::Integer(-1));
    assert_eq!(error.code, -32600);
    assert!(
        error
            .message
            .starts_with("failed to parse websocket JSON-RPC message from exec-server websocket"),
        "unexpected malformed-message error: {}",
        error.message
    );

    let initialize_id = server
        .send_request(
            "initialize",
            serde_json::to_value(InitializeParams {
                client_name: "exec-server-test".to_string(),
                resume_session_id: None,
            })?,
        )
        .await?;

    let response = server
        .wait_for_event(|event| {
            matches!(
                event,
                JSONRPCMessage::Response(JSONRPCResponse { id, .. }) if id == &initialize_id
            )
        })
        .await?;
    let JSONRPCMessage::Response(JSONRPCResponse { id, result }) = response else {
        panic!("expected initialize response after malformed input");
    };
    assert_eq!(id, initialize_id);
    let initialize_response: InitializeResponse = serde_json::from_value(result)?;
    Uuid::parse_str(&initialize_response.session_id)?;

    server.shutdown().await?;
    Ok(())
}
