use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_protocol::protocol::W3cTraceContext;
use prost::Message;

use crate::JSONRPCError;
use crate::JSONRPCErrorError;
use crate::JSONRPCMessage;
use crate::JSONRPCNotification;
use crate::JSONRPCRequest;
use crate::JSONRPCResponse;
use crate::RequestId;
use crate::proto::json;
use crate::proto::pb;
use crate::proto::pb::jsonrpc_message;
use crate::proto::pb::request_id;

pub fn encode_jsonrpc_message(message: JSONRPCMessage) -> Vec<u8> {
    pb::JsonrpcMessage::from(message).encode_to_vec()
}

pub fn decode_jsonrpc_message(bytes: &[u8]) -> Result<JSONRPCMessage> {
    let message = pb::JsonrpcMessage::decode(bytes).context("decode JSON-RPC protobuf message")?;
    JSONRPCMessage::try_from(message)
}

impl From<RequestId> for pb::RequestId {
    fn from(id: RequestId) -> Self {
        let kind = match id {
            RequestId::String(value) => request_id::Kind::StringValue(value),
            RequestId::Integer(value) => request_id::Kind::IntegerValue(value),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<pb::RequestId> for RequestId {
    type Error = anyhow::Error;

    fn try_from(id: pb::RequestId) -> Result<Self> {
        match id.kind.context("missing request ID kind")? {
            request_id::Kind::StringValue(value) => Ok(Self::String(value)),
            request_id::Kind::IntegerValue(value) => Ok(Self::Integer(value)),
        }
    }
}

impl From<JSONRPCRequest> for pb::JsonrpcRequest {
    fn from(request: JSONRPCRequest) -> Self {
        Self {
            id: Some(request.id.into()),
            method: request.method,
            params: request.params.map(json::serde_json_to_proto_value),
            trace: request.trace.map(|trace| pb::W3cTraceContext {
                traceparent: trace.traceparent,
                tracestate: trace.tracestate,
            }),
        }
    }
}

impl TryFrom<pb::JsonrpcRequest> for JSONRPCRequest {
    type Error = anyhow::Error;

    fn try_from(request: pb::JsonrpcRequest) -> Result<Self> {
        Ok(Self {
            id: request
                .id
                .context("missing request ID")?
                .try_into()
                .context("decode request ID")?,
            method: request.method,
            params: request.params.map(json::proto_value_to_serde_json),
            trace: request.trace.map(|trace| W3cTraceContext {
                traceparent: trace.traceparent,
                tracestate: trace.tracestate,
            }),
        })
    }
}

impl From<JSONRPCNotification> for pb::JsonrpcNotification {
    fn from(notification: JSONRPCNotification) -> Self {
        Self {
            method: notification.method,
            params: notification.params.map(json::serde_json_to_proto_value),
        }
    }
}

impl From<pb::JsonrpcNotification> for JSONRPCNotification {
    fn from(notification: pb::JsonrpcNotification) -> Self {
        Self {
            method: notification.method,
            params: notification.params.map(json::proto_value_to_serde_json),
        }
    }
}

impl From<JSONRPCResponse> for pb::JsonrpcResponse {
    fn from(response: JSONRPCResponse) -> Self {
        Self {
            id: Some(response.id.into()),
            result: Some(json::serde_json_to_proto_value(response.result)),
        }
    }
}

impl TryFrom<pb::JsonrpcResponse> for JSONRPCResponse {
    type Error = anyhow::Error;

    fn try_from(response: pb::JsonrpcResponse) -> Result<Self> {
        Ok(Self {
            id: response
                .id
                .context("missing response ID")?
                .try_into()
                .context("decode response ID")?,
            result: response
                .result
                .map(json::proto_value_to_serde_json)
                .context("missing response result")?,
        })
    }
}

impl From<JSONRPCErrorError> for pb::JsonrpcErrorError {
    fn from(error: JSONRPCErrorError) -> Self {
        Self {
            code: error.code,
            data: error.data.map(json::serde_json_to_proto_value),
            message_field: error.message,
        }
    }
}

impl From<pb::JsonrpcErrorError> for JSONRPCErrorError {
    fn from(error: pb::JsonrpcErrorError) -> Self {
        Self {
            code: error.code,
            data: error.data.map(json::proto_value_to_serde_json),
            message: error.message_field,
        }
    }
}

impl From<JSONRPCError> for pb::JsonrpcError {
    fn from(error: JSONRPCError) -> Self {
        Self {
            error: Some(error.error.into()),
            id: Some(error.id.into()),
        }
    }
}

impl TryFrom<pb::JsonrpcError> for JSONRPCError {
    type Error = anyhow::Error;

    fn try_from(error: pb::JsonrpcError) -> Result<Self> {
        Ok(Self {
            error: error
                .error
                .context("missing JSON-RPC error payload")?
                .into(),
            id: error
                .id
                .context("missing error ID")?
                .try_into()
                .context("decode error ID")?,
        })
    }
}

impl From<JSONRPCMessage> for pb::JsonrpcMessage {
    fn from(message: JSONRPCMessage) -> Self {
        let kind = match message {
            JSONRPCMessage::Request(request) => {
                jsonrpc_message::Kind::JsonrpcRequest(request.into())
            }
            JSONRPCMessage::Notification(notification) => {
                jsonrpc_message::Kind::JsonrpcNotification(notification.into())
            }
            JSONRPCMessage::Response(response) => {
                jsonrpc_message::Kind::JsonrpcResponse(response.into())
            }
            JSONRPCMessage::Error(error) => jsonrpc_message::Kind::JsonrpcError(error.into()),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<pb::JsonrpcMessage> for JSONRPCMessage {
    type Error = anyhow::Error;

    fn try_from(message: pb::JsonrpcMessage) -> Result<Self> {
        match message
            .kind
            .ok_or_else(|| anyhow!("missing JSON-RPC message kind"))?
        {
            jsonrpc_message::Kind::JsonrpcRequest(request) => {
                Ok(Self::Request(request.try_into()?))
            }
            jsonrpc_message::Kind::JsonrpcNotification(notification) => {
                Ok(Self::Notification(notification.into()))
            }
            jsonrpc_message::Kind::JsonrpcResponse(response) => {
                Ok(Self::Response(response.try_into()?))
            }
            jsonrpc_message::Kind::JsonrpcError(error) => Ok(Self::Error(error.try_into()?)),
        }
    }
}
