use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_extension_api::ToolCallError;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolHandler;
use codex_memories_mcp::LocalMemoriesBackend;
use codex_memories_mcp::backend::DEFAULT_READ_MAX_TOKENS;
use codex_memories_mcp::backend::MemoriesBackend;
use codex_memories_mcp::backend::ReadMemoryRequest;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde::Deserialize;
use serde_json::Value;

use super::backend_error;
use super::ensure_root;
use super::parse_arguments;
use super::serialize_output;

const TOOL_NAME: &str = "read_memory";

#[derive(Debug)]
pub(super) struct ReadMemoryTool {
    backend: LocalMemoriesBackend,
}

impl ReadMemoryTool {
    pub(super) fn new(backend: LocalMemoriesBackend) -> Self {
        Self { backend }
    }

    pub(super) fn contribution(self: &Arc<Self>) -> ToolContribution {
        let handler: Arc<dyn ToolHandler> = self.clone();
        ToolContribution::new(tool_spec(), handler).allow_parallel_calls()
    }
}

impl ToolHandler for ReadMemoryTool {
    fn handle<'a>(
        &'a self,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolCallError>> + Send + 'a>> {
        Box::pin(async move {
            let args: ReadArgs = parse_arguments(arguments)?;
            ensure_root(&self.backend).await?;
            let response = self
                .backend
                .read(ReadMemoryRequest {
                    path: args.path,
                    line_offset: args.line_offset.unwrap_or(1),
                    max_lines: args.max_lines,
                    max_tokens: DEFAULT_READ_MAX_TOKENS,
                })
                .await
                .map_err(backend_error)?;
            serialize_output(response)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    line_offset: Option<usize>,
    max_lines: Option<usize>,
}

fn tool_spec() -> ResponsesApiTool {
    let properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Relative path of the Codex memory file to read.".to_string(),
            )),
        ),
        (
            "line_offset".to_string(),
            JsonSchema::integer(Some(
                "Optional 1-indexed line number to start reading from.".to_string(),
            )),
        ),
        (
            "max_lines".to_string(),
            JsonSchema::integer(Some(
                "Optional maximum number of lines to return.".to_string(),
            )),
        ),
    ]);

    ResponsesApiTool {
        name: TOOL_NAME.to_string(),
        description:
            "Read a Codex memory file by relative path, optionally starting at a 1-indexed line offset and limiting the number of lines returned."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    }
}
