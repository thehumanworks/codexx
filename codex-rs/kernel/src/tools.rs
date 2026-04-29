use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolSpec;
use std::fmt;
use tokio_util::sync::CancellationToken;

/// Retry-stable tool registration for a logical model request.
///
/// The host chooses which tools are exposed to the model and whether the model
/// may issue them in parallel. `codex-kernel` treats this as opaque model-facing
/// configuration and uses it to rebuild prompts across retries.
#[derive(Debug, Clone, Default)]
pub struct ToolConfig {
    pub tools: Vec<ToolSpec>,
    pub parallel_tool_calls: bool,
}

/// Metadata a host-specific tool call must expose so `codex-kernel` can
/// synthesize a model-visible failure response when execution cannot complete.
pub trait KernelToolCall: Clone + Send + Sync {
    fn error_response(&self, message: String) -> ResponseInputItem;
}

/// Non-fatal tool execution failures are surfaced back to the model as a
/// complementary output item, while fatal failures abort the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallError {
    RespondToModel(String),
    Fatal(String),
}

impl fmt::Display for ToolCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RespondToModel(message) => write!(f, "{message}"),
            Self::Fatal(message) => write!(f, "Fatal error: {message}"),
        }
    }
}

/// Classification of a completed model output item relative to the host's tool
/// catalog.
pub enum CompletedResponseItem<Call> {
    NonTool(ResponseItem),
    ToolCall {
        item: ResponseItem,
        call: Call,
    },
    ImmediateResponse {
        item: ResponseItem,
        response: ResponseInputItem,
    },
}

/// Host-provided tool adapter used by `codex-kernel`'s default model/tool loop.
///
/// Implementations own tool-call parsing and execution, while
/// `codex-kernel` owns the generic behavior of attempting a complementary
/// tool output item for every tool call the model emits.
#[allow(async_fn_in_trait)]
pub trait KernelToolExecutor {
    type Call: KernelToolCall;

    async fn classify_response_item(
        &self,
        item: ResponseItem,
    ) -> Result<CompletedResponseItem<Self::Call>>;

    async fn execute_tool_call(
        &self,
        call: Self::Call,
        cancellation_token: CancellationToken,
    ) -> std::result::Result<ResponseInputItem, ToolCallError>;
}

pub async fn execute_tool_call_with_default_output<Executor>(
    executor: &Executor,
    call: Executor::Call,
    cancellation_token: CancellationToken,
) -> Result<ResponseInputItem>
where
    Executor: KernelToolExecutor,
{
    match executor
        .execute_tool_call(call.clone(), cancellation_token)
        .await
    {
        Ok(response) => Ok(response),
        Err(ToolCallError::RespondToModel(message)) => Ok(call.error_response(message)),
        Err(ToolCallError::Fatal(message)) => Err(CodexErr::Fatal(message)),
    }
}

pub fn response_input_to_response_item(input: &ResponseInputItem) -> Option<ResponseItem> {
    match input {
        ResponseInputItem::FunctionCallOutput { call_id, output } => {
            Some(ResponseItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output: output.clone(),
            })
        }
        ResponseInputItem::CustomToolCallOutput {
            call_id,
            name,
            output,
        } => Some(ResponseItem::CustomToolCallOutput {
            call_id: call_id.clone(),
            name: name.clone(),
            output: output.clone(),
        }),
        ResponseInputItem::McpToolCallOutput { call_id, output } => {
            let output = output.as_function_call_output_payload();
            Some(ResponseItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output,
            })
        }
        ResponseInputItem::ToolSearchOutput {
            call_id,
            status,
            execution,
            tools,
        } => Some(ResponseItem::ToolSearchOutput {
            call_id: Some(call_id.clone()),
            status: status.clone(),
            execution: execution.clone(),
            tools: tools.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::CompletedResponseItem;
    use super::KernelToolCall;
    use super::KernelToolExecutor;
    use super::ToolCallError;
    use super::execute_tool_call_with_default_output;
    use codex_protocol::error::Result;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseInputItem;
    use codex_protocol::models::ResponseItem;
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestCall {
        call_id: String,
    }

    impl KernelToolCall for TestCall {
        fn error_response(&self, message: String) -> ResponseInputItem {
            ResponseInputItem::FunctionCallOutput {
                call_id: self.call_id.clone(),
                output: FunctionCallOutputPayload::from_text(message),
            }
        }
    }

    struct TestExecutor;

    impl KernelToolExecutor for TestExecutor {
        type Call = TestCall;

        async fn classify_response_item(
            &self,
            item: ResponseItem,
        ) -> Result<CompletedResponseItem<Self::Call>> {
            Ok(CompletedResponseItem::NonTool(item))
        }

        async fn execute_tool_call(
            &self,
            call: Self::Call,
            _cancellation_token: CancellationToken,
        ) -> std::result::Result<ResponseInputItem, ToolCallError> {
            match call.call_id.as_str() {
                "ok" => Ok(ResponseInputItem::FunctionCallOutput {
                    call_id: call.call_id,
                    output: FunctionCallOutputPayload::from_text("done".to_string()),
                }),
                "retry" => Err(ToolCallError::RespondToModel("try again".to_string())),
                _ => Err(ToolCallError::Fatal("boom".to_string())),
            }
        }
    }

    #[tokio::test]
    async fn execute_tool_call_with_default_output_surfaces_non_fatal_errors() {
        let response = execute_tool_call_with_default_output(
            &TestExecutor,
            TestCall {
                call_id: "retry".to_string(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("tool response");

        assert_eq!(
            response,
            ResponseInputItem::FunctionCallOutput {
                call_id: "retry".to_string(),
                output: FunctionCallOutputPayload::from_text("try again".to_string()),
            }
        );
    }

    #[tokio::test]
    async fn execute_tool_call_with_default_output_propagates_fatal_errors() {
        let err = execute_tool_call_with_default_output(
            &TestExecutor,
            TestCall {
                call_id: "fatal".to_string(),
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("fatal tool error");

        assert_eq!(err.to_string(), "Fatal error: boom");
    }
}
