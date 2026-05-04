use anyhow::Result;
use codex_app_server_protocol::ChatgptAuthTokensRefreshParams;
use codex_app_server_protocol::ChatgptAuthTokensRefreshReason;
use codex_app_server_protocol::ChatgptAuthTokensRefreshResponse;
use codex_app_server_protocol::ClientNotification;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ClientResponse;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ServerResponse;
use codex_app_server_protocol::ThreadArchiveParams;
use codex_app_server_protocol::ThreadArchiveResponse;
use codex_app_server_protocol::ThreadClosedNotification;
use codex_app_server_protocol::proto::jsonrpc;
use pretty_assertions::assert_eq;
use serde_json::json as json_value;

fn round_trip_jsonrpc_message(message: JSONRPCMessage) -> Result<JSONRPCMessage> {
    let encoded = jsonrpc::encode_jsonrpc_message(message);
    jsonrpc::decode_jsonrpc_message(&encoded)
}

#[test]
fn client_request_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ClientRequest::ThreadArchive {
        request_id: RequestId::Integer(1),
        params: ThreadArchiveParams {
            thread_id: "thread-1".to_string(),
        },
    };

    let jsonrpc = original
        .clone()
        .into_jsonrpc_request()
        .expect("convert to JSON-RPC request");
    let round_tripped_jsonrpc = match round_trip_jsonrpc_message(JSONRPCMessage::Request(jsonrpc))?
    {
        JSONRPCMessage::Request(request) => request,
        _ => panic!("expected request"),
    };
    let round_tripped =
        ClientRequest::from_jsonrpc_request(round_tripped_jsonrpc).expect("parse client request");

    assert_eq!(
        serde_json::to_value(round_tripped).expect("serialize round-tripped request"),
        serde_json::to_value(original).expect("serialize original request")
    );

    Ok(())
}

#[test]
fn client_response_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ClientResponse::ThreadArchive {
        request_id: RequestId::Integer(1),
        response: ThreadArchiveResponse {},
    };
    let jsonrpc = original
        .into_jsonrpc_response()
        .expect("convert to JSON-RPC response");

    let round_tripped = match round_trip_jsonrpc_message(JSONRPCMessage::Response(jsonrpc.clone()))?
    {
        JSONRPCMessage::Response(response) => response,
        _ => panic!("expected response"),
    };

    assert_eq!(round_tripped, jsonrpc);

    Ok(())
}

#[test]
fn server_request_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ServerRequest::ChatgptAuthTokensRefresh {
        request_id: RequestId::String("server-request-1".to_string()),
        params: ChatgptAuthTokensRefreshParams {
            reason: ChatgptAuthTokensRefreshReason::Unauthorized,
            previous_account_id: None,
        },
    };

    let jsonrpc = original
        .clone()
        .into_jsonrpc_request()
        .expect("convert to JSON-RPC request");
    let round_tripped_jsonrpc = match round_trip_jsonrpc_message(JSONRPCMessage::Request(jsonrpc))?
    {
        JSONRPCMessage::Request(request) => request,
        _ => panic!("expected request"),
    };
    let round_tripped =
        ServerRequest::try_from(round_tripped_jsonrpc).expect("parse server request");

    assert_eq!(
        serde_json::to_value(round_tripped).expect("serialize round-tripped request"),
        serde_json::to_value(original).expect("serialize original request")
    );

    Ok(())
}

#[test]
fn server_response_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ServerResponse::ChatgptAuthTokensRefresh {
        request_id: RequestId::String("server-request-1".to_string()),
        response: ChatgptAuthTokensRefreshResponse {
            access_token: "access-token".to_string(),
            chatgpt_account_id: "account-1".to_string(),
            chatgpt_plan_type: None,
        },
    };
    let jsonrpc = original
        .into_jsonrpc_response()
        .expect("convert to JSON-RPC response");

    let round_tripped = match round_trip_jsonrpc_message(JSONRPCMessage::Response(jsonrpc.clone()))?
    {
        JSONRPCMessage::Response(response) => response,
        _ => panic!("expected response"),
    };

    assert_eq!(round_tripped, jsonrpc);

    Ok(())
}

#[test]
fn server_notification_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ServerNotification::ThreadClosed(ThreadClosedNotification {
        thread_id: "thread-1".to_string(),
    });

    let jsonrpc = original
        .clone()
        .into_jsonrpc_notification()
        .expect("convert to JSON-RPC notification");
    let round_tripped_jsonrpc =
        match round_trip_jsonrpc_message(JSONRPCMessage::Notification(jsonrpc))? {
            JSONRPCMessage::Notification(notification) => notification,
            _ => panic!("expected notification"),
        };
    let round_tripped =
        ServerNotification::try_from(round_tripped_jsonrpc).expect("parse server notification");

    assert_eq!(
        serde_json::to_value(round_tripped).expect("serialize round-tripped notification"),
        serde_json::to_value(original).expect("serialize original notification")
    );

    Ok(())
}

#[test]
fn client_notification_round_trips_through_protobuf_wire() -> Result<()> {
    let original = ClientNotification::Initialized;
    let jsonrpc = serde_json::from_value::<JSONRPCNotification>(
        serde_json::to_value(&original).expect("serialize client notification"),
    )
    .expect("convert to JSON-RPC notification");

    let round_tripped_jsonrpc =
        match round_trip_jsonrpc_message(JSONRPCMessage::Notification(jsonrpc))? {
            JSONRPCMessage::Notification(notification) => notification,
            _ => panic!("expected notification"),
        };
    let round_tripped = serde_json::from_value::<ClientNotification>(
        serde_json::to_value(round_tripped_jsonrpc).expect("serialize JSON-RPC notification"),
    )
    .expect("parse client notification");

    assert_eq!(
        serde_json::to_value(round_tripped).expect("serialize round-tripped notification"),
        serde_json::to_value(original).expect("serialize original notification")
    );

    Ok(())
}

#[test]
fn jsonrpc_error_round_trips_through_protobuf_wire() -> Result<()> {
    let original = JSONRPCError {
        id: RequestId::Integer(1),
        error: JSONRPCErrorError {
            code: -32601,
            message: "method not found".to_string(),
            data: Some(json_value!({ "method": "missing/method" })),
        },
    };

    let round_tripped = match round_trip_jsonrpc_message(JSONRPCMessage::Error(original.clone()))? {
        JSONRPCMessage::Error(error) => error,
        _ => panic!("expected error"),
    };

    assert_eq!(round_tripped, original);

    Ok(())
}
