use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::SkillLoadOutcome;
use crate::client::ModelClientSession;
use crate::session::session::Session;
use crate::session::turn::build_prompt_config;
use crate::session::turn::build_tool_config;
use crate::session::turn::built_tools;
use crate::session::turn::try_run_sampling_request;
use crate::session::turn_context::TurnContext;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::parallel::ToolCallRuntime;
use codex_kernel::PreparedSamplingRequest;
use codex_kernel::Prompt;
use codex_kernel::SamplingLoopHost;
use codex_kernel::SamplingRequestResult;
use codex_kernel::run_sampling_request_loop;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RateLimitSnapshot;
use codex_protocol::protocol::StreamErrorEvent;
use codex_protocol::protocol::WarningEvent;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use tracing::warn;

struct SamplingRequestRuntime {
    _code_mode_worker: Option<codex_code_mode::CodeModeTurnWorker>,
}

#[allow(clippy::too_many_arguments)]
#[instrument(
    level = "trace",
    skip_all,
    fields(
        turn_id = %turn_context.sub_id,
        model = %turn_context.model_info.slug,
        cwd = %turn_context.cwd.display()
    )
)]
pub(super) async fn run_sampling_request(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    turn_diff_tracker: SharedTurnDiffTracker,
    client_session: &mut ModelClientSession,
    turn_metadata_header: Option<&str>,
    input: Vec<ResponseItem>,
    explicitly_enabled_connectors: &HashSet<String>,
    skills_outcome: Option<&SkillLoadOutcome>,
    cancellation_token: CancellationToken,
) -> CodexResult<SamplingRequestResult> {
    let host = CoreSamplingLoopHost {
        sess,
        turn_context,
        turn_diff_tracker,
        turn_metadata_header,
        explicitly_enabled_connectors,
        skills_outcome,
    };

    run_sampling_request_loop(&host, client_session, input, cancellation_token).await
}

struct CoreSamplingLoopHost<'a> {
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    turn_diff_tracker: SharedTurnDiffTracker,
    turn_metadata_header: Option<&'a str>,
    explicitly_enabled_connectors: &'a HashSet<String>,
    skills_outcome: Option<&'a SkillLoadOutcome>,
}

impl SamplingLoopHost for CoreSamplingLoopHost<'_> {
    type ClientSession = ModelClientSession;
    type Runtime = SamplingRequestRuntime;
    type Tools = ToolCallRuntime;

    async fn prepare_sampling_request(
        &self,
        input: &[ResponseItem],
        cancellation_token: &CancellationToken,
    ) -> CodexResult<PreparedSamplingRequest<Self::Runtime, Self::Tools>> {
        let router = built_tools(
            self.sess.as_ref(),
            self.turn_context.as_ref(),
            input,
            self.explicitly_enabled_connectors,
            self.skills_outcome,
            cancellation_token,
        )
        .await?;
        let base_instructions = self.sess.get_base_instructions().await;
        let tool_runtime = ToolCallRuntime::new(
            Arc::clone(&router),
            Arc::clone(&self.sess),
            Arc::clone(&self.turn_context),
            Arc::clone(&self.turn_diff_tracker),
        );
        let code_mode_worker = self
            .sess
            .services
            .code_mode_service
            .start_turn_worker(
                &self.sess,
                &self.turn_context,
                Arc::clone(&router),
                Arc::clone(&self.turn_diff_tracker),
            )
            .await;

        Ok(PreparedSamplingRequest {
            prompt_config: build_prompt_config(self.turn_context.as_ref(), base_instructions),
            tool_config: build_tool_config(router.as_ref(), self.turn_context.as_ref()),
            runtime: SamplingRequestRuntime {
                _code_mode_worker: code_mode_worker,
            },
            tools: tool_runtime,
        })
    }

    async fn history_prompt_input(&self) -> Vec<ResponseItem> {
        self.sess
            .clone_history()
            .await
            .for_prompt(&self.turn_context.model_info.input_modalities)
    }

    async fn run_single_sampling_request(
        &self,
        _runtime: &Self::Runtime,
        tools: &Self::Tools,
        client_session: &mut Self::ClientSession,
        prompt: &Prompt,
        cancellation_token: CancellationToken,
    ) -> CodexResult<SamplingRequestResult> {
        try_run_sampling_request(
            tools.clone(),
            Arc::clone(&self.sess),
            Arc::clone(&self.turn_context),
            client_session,
            self.turn_metadata_header,
            Arc::clone(&self.turn_diff_tracker),
            prompt,
            cancellation_token,
        )
        .await
    }

    fn stream_max_retries(&self) -> u64 {
        self.turn_context.provider.info().stream_max_retries()
    }

    fn try_switch_fallback_transport(&self, client_session: &mut Self::ClientSession) -> bool {
        client_session.try_switch_fallback_transport(
            &self.turn_context.session_telemetry,
            &self.turn_context.model_info,
        )
    }

    fn should_notify_stream_retry(&self, retries: u64, _err: &CodexErr) -> bool {
        retries > 1
            || cfg!(debug_assertions)
            || !self
                .sess
                .services
                .model_client
                .responses_websocket_enabled()
    }

    async fn handle_context_window_exceeded(&self) {
        self.sess.set_total_tokens_full(&self.turn_context).await;
    }

    async fn handle_usage_limit_reached(&self, rate_limits: Option<RateLimitSnapshot>) {
        if let Some(rate_limits) = rate_limits {
            self.sess
                .update_rate_limits(&self.turn_context, rate_limits)
                .await;
        }
    }

    async fn notify_fallback_to_http(&self, err: &CodexErr) {
        self.sess
            .send_event(
                &self.turn_context,
                EventMsg::Warning(WarningEvent {
                    message: format!("Falling back from WebSockets to HTTPS transport. {err:#}"),
                }),
            )
            .await;
    }

    async fn notify_stream_retry(
        &self,
        retries: u64,
        max_retries: u64,
        delay: Duration,
        err: &CodexErr,
    ) {
        warn!(
            "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})..."
        );
        self.sess
            .send_event(
                &self.turn_context,
                EventMsg::StreamError(StreamErrorEvent {
                    message: format!("Reconnecting... {retries}/{max_retries}"),
                    codex_error_info: Some(CodexErrorInfo::ResponseStreamDisconnected {
                        http_status_code: err.http_status_code_value(),
                    }),
                    additional_details: Some(err.to_string()),
                }),
            )
            .await;
    }
}
