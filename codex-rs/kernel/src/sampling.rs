use crate::KernelToolExecutor;
use crate::Prompt;
use crate::PromptConfig;
use crate::ToolConfig;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::RateLimitSnapshot;
use rand::Rng;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const INITIAL_DELAY_MS: u64 = 200;
const BACKOFF_FACTOR: f64 = 2.0;

/// Host-facing bundle created once before the kernel begins retrying a logical sampling request.
///
/// Implementations should place retry-stable state here so retries can rebuild prompts without
/// repeating expensive setup work such as tool routing or per-turn worker startup.
pub struct PreparedSamplingRequest<Runtime, Tools> {
    pub prompt_config: PromptConfig,
    pub tool_config: ToolConfig,
    pub runtime: Runtime,
    pub tools: Tools,
}

/// Final outcome of a logical sampling request after the model/tool loop reaches a stable state.
#[derive(Debug)]
pub struct SamplingRequestResult {
    pub needs_follow_up: bool,
    pub last_agent_message: Option<String>,
}

/// Host adapter for the kernel's retryable sampling loop.
///
/// Implementations provide the concrete model transport, tool runtime, history access, and
/// user-facing retry notifications while allowing `codex-kernel` to stay agnostic of local
/// filesystem, network, or tool-execution details.
#[allow(async_fn_in_trait)]
pub trait SamplingLoopHost {
    type ClientSession;
    type Runtime;
    type Tools: KernelToolExecutor;

    async fn prepare_sampling_request(
        &self,
        input: &[ResponseItem],
        cancellation_token: &CancellationToken,
    ) -> Result<PreparedSamplingRequest<Self::Runtime, Self::Tools>>;

    async fn history_prompt_input(&self) -> Vec<ResponseItem>;

    async fn run_single_sampling_request(
        &self,
        runtime: &Self::Runtime,
        tools: &Self::Tools,
        client_session: &mut Self::ClientSession,
        prompt: &Prompt,
        cancellation_token: CancellationToken,
    ) -> Result<SamplingRequestResult>;

    fn stream_max_retries(&self) -> u64;

    fn try_switch_fallback_transport(&self, client_session: &mut Self::ClientSession) -> bool;

    fn should_notify_stream_retry(&self, retries: u64, err: &CodexErr) -> bool;

    async fn handle_context_window_exceeded(&self);

    async fn handle_usage_limit_reached(&self, rate_limits: Option<RateLimitSnapshot>);

    async fn notify_fallback_to_http(&self, err: &CodexErr);

    async fn notify_stream_retry(
        &self,
        retries: u64,
        max_retries: u64,
        delay: Duration,
        err: &CodexErr,
    );
}

pub async fn run_sampling_request_loop<Host>(
    host: &Host,
    client_session: &mut Host::ClientSession,
    initial_input: Vec<ResponseItem>,
    cancellation_token: CancellationToken,
) -> Result<SamplingRequestResult>
where
    Host: SamplingLoopHost,
{
    let prepared = host
        .prepare_sampling_request(initial_input.as_slice(), &cancellation_token)
        .await?;
    let mut retries = 0;
    let mut initial_input = Some(initial_input);
    loop {
        let prompt_input = if let Some(input) = initial_input.take() {
            input
        } else {
            host.history_prompt_input().await
        };
        let prompt = prepared
            .prompt_config
            .build_prompt(prompt_input, &prepared.tool_config);
        let err = match host
            .run_single_sampling_request(
                &prepared.runtime,
                &prepared.tools,
                client_session,
                &prompt,
                cancellation_token.child_token(),
            )
            .await
        {
            Ok(output) => {
                return Ok(output);
            }
            Err(CodexErr::ContextWindowExceeded) => {
                host.handle_context_window_exceeded().await;
                return Err(CodexErr::ContextWindowExceeded);
            }
            Err(CodexErr::UsageLimitReached(error)) => {
                host.handle_usage_limit_reached(
                    error
                        .rate_limits
                        .as_ref()
                        .map(|snapshot| (**snapshot).clone()),
                )
                .await;
                return Err(CodexErr::UsageLimitReached(error));
            }
            Err(err) => err,
        };

        if !err.is_retryable() {
            return Err(err);
        }

        let max_retries = host.stream_max_retries();
        if retries >= max_retries && host.try_switch_fallback_transport(client_session) {
            host.notify_fallback_to_http(&err).await;
            retries = 0;
            continue;
        }
        if retries < max_retries {
            retries += 1;
            let delay = match &err {
                CodexErr::Stream(_, requested_delay) => {
                    requested_delay.unwrap_or_else(|| backoff(retries))
                }
                _ => backoff(retries),
            };
            if host.should_notify_stream_retry(retries, &err) {
                host.notify_stream_retry(retries, max_retries, delay, &err)
                    .await;
            }
            tokio::time::sleep(delay).await;
        } else {
            return Err(err);
        }
    }
}

fn backoff(attempt: u64) -> Duration {
    let exp = BACKOFF_FACTOR.powi(attempt.saturating_sub(1) as i32);
    let base = (INITIAL_DELAY_MS as f64 * exp) as u64;
    let jitter = rand::rng().random_range(0.9..1.1);
    Duration::from_millis((base as f64 * jitter) as u64)
}
