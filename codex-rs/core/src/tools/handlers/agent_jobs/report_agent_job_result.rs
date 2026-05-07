use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_tools::ToolName;

use super::*;

pub struct ReportAgentJobResultHandler;

impl ToolHandler for ReportAgentJobResultHandler {
    type Output = FunctionToolOutput;

    fn tool_name(&self) -> ToolName {
        ToolName::plain("report_agent_job_result")
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session, payload, ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "report_agent_job_result handler received unsupported payload".to_string(),
                ));
            }
        };

        handle(session, arguments).await
    }
}

pub async fn handle(
    session: Arc<Session>,
    arguments: String,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let args: ReportAgentJobResultArgs = parse_arguments(arguments.as_str())?;
    if !args.result.is_object() {
        return Err(FunctionCallError::RespondToModel(
            "result must be a JSON object".to_string(),
        ));
    }
    let db = required_state_db(&session)?;
    let reporting_thread_id = session.conversation_id.to_string();
    let accepted = if args.stop.unwrap_or(false) {
        db.report_agent_job_item_result_and_cancel_job(
            args.job_id.as_str(),
            args.item_id.as_str(),
            reporting_thread_id.as_str(),
            &args.result,
            "cancelled by worker request",
        )
        .await
    } else {
        db.report_agent_job_item_result(
            args.job_id.as_str(),
            args.item_id.as_str(),
            reporting_thread_id.as_str(),
            &args.result,
        )
        .await
    }
    .map_err(|err| {
        let job_id = args.job_id.as_str();
        let item_id = args.item_id.as_str();
        FunctionCallError::RespondToModel(format!(
            "failed to record agent job result for {job_id} / {item_id}: {err}"
        ))
    })?;
    let content =
        serde_json::to_string(&ReportAgentJobResultToolResult { accepted }).map_err(|err| {
            FunctionCallError::Fatal(format!(
                "failed to serialize report_agent_job_result result: {err}"
            ))
        })?;
    Ok(FunctionToolOutput::from_text(content, Some(true)))
}
