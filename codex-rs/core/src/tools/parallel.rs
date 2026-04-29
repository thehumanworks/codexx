use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio_util::either::Either;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::Instrument;
use tracing::instrument;
use tracing::trace_span;

use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::AbortedToolOutput;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolPayload;
use crate::tools::registry::AnyToolResult;
use crate::tools::registry::ToolArgumentDiffConsumer;
use crate::tools::router::ToolCall;
use crate::tools::router::ToolCallSource;
use crate::tools::router::ToolRouter;
use codex_kernel::CompletedResponseItem;
use codex_kernel::KernelToolCall;
use codex_kernel::KernelToolExecutor;
use codex_kernel::ToolCallError;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolSpec;

#[derive(Clone)]
pub(crate) struct ToolCallRuntime {
    router: Arc<ToolRouter>,
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    tracker: SharedTurnDiffTracker,
    parallel_execution: Arc<RwLock<()>>,
}

impl ToolCallRuntime {
    pub(crate) fn new(
        router: Arc<ToolRouter>,
        session: Arc<Session>,
        turn_context: Arc<TurnContext>,
        tracker: SharedTurnDiffTracker,
    ) -> Self {
        Self {
            router,
            session,
            turn_context,
            tracker,
            parallel_execution: Arc::new(RwLock::new(())),
        }
    }

    pub(crate) fn find_spec(&self, tool_name: &codex_tools::ToolName) -> Option<ToolSpec> {
        self.router.find_spec(tool_name)
    }

    pub(crate) fn create_diff_consumer(
        &self,
        tool_name: &codex_tools::ToolName,
    ) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        self.router.create_diff_consumer(tool_name)
    }

    #[instrument(level = "trace", skip_all)]
    pub(crate) fn handle_tool_call_with_source(
        self,
        call: ToolCall,
        source: ToolCallSource,
        cancellation_token: CancellationToken,
    ) -> impl std::future::Future<Output = Result<AnyToolResult, FunctionCallError>> {
        let supports_parallel = self.router.tool_supports_parallel(&call);
        let router = Arc::clone(&self.router);
        let session = Arc::clone(&self.session);
        let turn = Arc::clone(&self.turn_context);
        let tracker = Arc::clone(&self.tracker);
        let lock = Arc::clone(&self.parallel_execution);
        let invocation_cancellation_token = cancellation_token.clone();
        let started = Instant::now();
        let display_name = call.tool_name.display();

        let dispatch_span = trace_span!(
            "dispatch_tool_call_with_code_mode_result",
            otel.name = display_name.as_str(),
            tool_name = display_name.as_str(),
            call_id = call.call_id.as_str(),
            aborted = false,
        );

        let handle: AbortOnDropHandle<Result<AnyToolResult, FunctionCallError>> =
            AbortOnDropHandle::new(tokio::spawn(async move {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        let secs = started.elapsed().as_secs_f32().max(0.1);
                        dispatch_span.record("aborted", true);
                        Ok(Self::aborted_response(&call, secs))
                    },
                    res = async {
                        let _guard = if supports_parallel {
                            Either::Left(lock.read().await)
                        } else {
                            Either::Right(lock.write().await)
                        };

                        router
                            .dispatch_tool_call_with_code_mode_result(
                                session,
                                turn,
                                invocation_cancellation_token,
                                tracker,
                                call.clone(),
                                source,
                            )
                            .instrument(dispatch_span.clone())
                            .await
                    } => res,
                }
            }));

        async move {
            handle.await.map_err(|err| {
                FunctionCallError::Fatal(format!("tool task failed to receive: {err:?}"))
            })?
        }
        .in_current_span()
    }
}

impl ToolCallRuntime {
    fn aborted_response(call: &ToolCall, secs: f32) -> AnyToolResult {
        AnyToolResult {
            call_id: call.call_id.clone(),
            payload: call.payload.clone(),
            result: Box::new(AbortedToolOutput {
                message: Self::abort_message(call, secs),
            }),
            post_tool_use_payload: None,
        }
    }

    fn abort_message(call: &ToolCall, secs: f32) -> String {
        if call.tool_name.namespace.is_none()
            && matches!(
                call.tool_name.name.as_str(),
                "shell" | "container.exec" | "local_shell" | "shell_command" | "unified_exec"
            )
        {
            format!("Wall time: {secs:.1} seconds\naborted by user")
        } else {
            format!("aborted by user after {secs:.1}s")
        }
    }
}

impl KernelToolCall for ToolCall {
    fn error_response(&self, message: String) -> ResponseInputItem {
        failure_response(self, message)
    }
}

impl KernelToolExecutor for ToolCallRuntime {
    type Call = ToolCall;

    async fn classify_response_item(
        &self,
        item: ResponseItem,
    ) -> CodexResult<CompletedResponseItem<Self::Call>> {
        match ToolRouter::build_tool_call(self.session.as_ref(), item.clone()).await {
            Ok(Some(call)) => Ok(CompletedResponseItem::ToolCall { item, call }),
            Ok(None) => Ok(CompletedResponseItem::NonTool(item)),
            Err(FunctionCallError::MissingLocalShellCallId) => {
                let response = ResponseInputItem::FunctionCallOutput {
                    call_id: String::new(),
                    output: codex_protocol::models::FunctionCallOutputPayload::from_text(
                        "LocalShellCall without call_id or id".to_string(),
                    ),
                };
                Ok(CompletedResponseItem::ImmediateResponse { item, response })
            }
            Err(FunctionCallError::RespondToModel(message)) => {
                let response = ResponseInputItem::FunctionCallOutput {
                    call_id: String::new(),
                    output: codex_protocol::models::FunctionCallOutputPayload::from_text(message),
                };
                Ok(CompletedResponseItem::ImmediateResponse { item, response })
            }
            Err(FunctionCallError::Fatal(message)) => Err(CodexErr::Fatal(message)),
        }
    }

    async fn execute_tool_call(
        &self,
        call: Self::Call,
        cancellation_token: CancellationToken,
    ) -> Result<ResponseInputItem, ToolCallError> {
        match self
            .clone()
            .handle_tool_call_with_source(call, ToolCallSource::Direct, cancellation_token)
            .await
        {
            Ok(result) => Ok(result.into_response()),
            Err(FunctionCallError::RespondToModel(message)) => {
                Err(ToolCallError::RespondToModel(message))
            }
            Err(FunctionCallError::MissingLocalShellCallId) => Err(ToolCallError::RespondToModel(
                "LocalShellCall without call_id or id".to_string(),
            )),
            Err(FunctionCallError::Fatal(message)) => Err(ToolCallError::Fatal(message)),
        }
    }
}

fn failure_response(call: &ToolCall, message: String) -> ResponseInputItem {
    match &call.payload {
        ToolPayload::ToolSearch { .. } => ResponseInputItem::ToolSearchOutput {
            call_id: call.call_id.clone(),
            status: "completed".to_string(),
            execution: "client".to_string(),
            tools: Vec::new(),
        },
        ToolPayload::Custom { .. } => ResponseInputItem::CustomToolCallOutput {
            call_id: call.call_id.clone(),
            name: None,
            output: codex_protocol::models::FunctionCallOutputPayload {
                body: codex_protocol::models::FunctionCallOutputBody::Text(message),
                success: Some(false),
            },
        },
        _ => ResponseInputItem::FunctionCallOutput {
            call_id: call.call_id.clone(),
            output: codex_protocol::models::FunctionCallOutputPayload {
                body: codex_protocol::models::FunctionCallOutputBody::Text(message),
                success: Some(false),
            },
        },
    }
}
