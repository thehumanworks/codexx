use super::*;
use anyhow::Result;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn client_response_payload_returns_jsonrpc_parts_and_client_response() -> Result<()> {
    let (request_id, result, payload) =
        ClientResponsePayload::ThreadArchive(v2::ThreadArchiveResponse {})
            .into_jsonrpc_parts_and_payload(RequestId::Integer(7))?;

    assert_eq!(request_id, RequestId::Integer(7));
    assert_eq!(result, json!({}));

    let Some(ClientResponse::ThreadArchive {
        request_id,
        response: _,
    }) = payload.and_then(|payload| payload.into_client_response(RequestId::Integer(7)))
    else {
        panic!("expected thread/archive client response");
    };
    assert_eq!(request_id, RequestId::Integer(7));
    Ok(())
}

#[test]
fn interrupt_conversation_payload_stays_jsonrpc_only() -> Result<()> {
    let (request_id, result, payload) =
        ClientResponsePayload::InterruptConversation(v1::InterruptConversationResponse {
            abort_reason: TurnAbortReason::Interrupted,
        })
        .into_jsonrpc_parts_and_payload(RequestId::Integer(8))?;

    assert_eq!(request_id, RequestId::Integer(8));
    assert_eq!(
        result,
        json!({
            "abortReason": "interrupted",
        })
    );
    assert!(payload.is_none());
    Ok(())
}

#[test]
fn client_request_method_comes_from_proto_registry() {
    let request = ClientRequest::ThreadArchive {
        request_id: RequestId::Integer(1),
        params: v2::ThreadArchiveParams {
            thread_id: "thread-1".to_string(),
        },
    };

    assert_eq!(request.variant_name(), "ThreadArchive");
    assert_eq!(request.method(), "thread/archive");
}

#[test]
fn client_request_jsonrpc_shim_preserves_legacy_wire_shape() -> Result<()> {
    let request = JSONRPCRequest {
        id: RequestId::Integer(1),
        method: "thread/archive".to_string(),
        params: Some(json!({ "threadId": "thread-1" })),
        trace: None,
    };

    let parsed = ClientRequest::from_jsonrpc_request(request)?;

    assert_eq!(
        parsed,
        ClientRequest::ThreadArchive {
            request_id: RequestId::Integer(1),
            params: v2::ThreadArchiveParams {
                thread_id: "thread-1".to_string(),
            },
        }
    );
    Ok(())
}

#[test]
fn client_request_into_jsonrpc_request_uses_proto_method_registry() -> Result<()> {
    let request = ClientRequest::ThreadArchive {
        request_id: RequestId::Integer(1),
        params: v2::ThreadArchiveParams {
            thread_id: "thread-1".to_string(),
        },
    };

    let jsonrpc = request.into_jsonrpc_request()?;

    assert_eq!(
        jsonrpc,
        JSONRPCRequest {
            id: RequestId::Integer(1),
            method: "thread/archive".to_string(),
            params: Some(json!({ "threadId": "thread-1" })),
            trace: None,
        }
    );
    Ok(())
}

#[test]
fn client_response_into_jsonrpc_response_preserves_result_shape() -> Result<()> {
    let response = ClientResponse::ThreadArchive {
        request_id: RequestId::Integer(1),
        response: v2::ThreadArchiveResponse {},
    };

    let jsonrpc = response.into_jsonrpc_response()?;

    assert_eq!(
        jsonrpc,
        JSONRPCResponse {
            id: RequestId::Integer(1),
            result: json!({}),
        }
    );
    Ok(())
}

#[test]
fn server_request_and_response_jsonrpc_shims_preserve_wire_shape() -> Result<()> {
    let request = ServerRequest::ChatgptAuthTokensRefresh {
        request_id: RequestId::String("server-request-1".to_string()),
        params: v2::ChatgptAuthTokensRefreshParams {
            reason: v2::ChatgptAuthTokensRefreshReason::Unauthorized,
            previous_account_id: None,
        },
    };

    let jsonrpc_request = request.into_jsonrpc_request()?;

    assert_eq!(
        jsonrpc_request,
        JSONRPCRequest {
            id: RequestId::String("server-request-1".to_string()),
            method: "account/chatgptAuthTokens/refresh".to_string(),
            params: Some(json!({
                "reason": "unauthorized",
                "previousAccountId": null,
            })),
            trace: None,
        }
    );

    let response = ServerResponse::ChatgptAuthTokensRefresh {
        request_id: RequestId::String("server-request-1".to_string()),
        response: v2::ChatgptAuthTokensRefreshResponse {
            access_token: "access-token".to_string(),
            chatgpt_account_id: "account-1".to_string(),
            chatgpt_plan_type: None,
        },
    };

    let jsonrpc_response = response.into_jsonrpc_response()?;

    assert_eq!(
        jsonrpc_response,
        JSONRPCResponse {
            id: RequestId::String("server-request-1".to_string()),
            result: json!({
                "accessToken": "access-token",
                "chatgptAccountId": "account-1",
                "chatgptPlanType": null,
            }),
        }
    );
    Ok(())
}

#[test]
fn server_notification_jsonrpc_shim_uses_proto_method_registry() -> Result<()> {
    let notification = ServerNotification::ThreadClosed(v2::ThreadClosedNotification {
        thread_id: "thread-1".to_string(),
    });

    let jsonrpc = notification.into_jsonrpc_notification()?;

    assert_eq!(jsonrpc.method, "thread/closed");
    assert_eq!(jsonrpc.params, Some(json!({ "threadId": "thread-1" })));
    Ok(())
}
