use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_extension_api::ToolCallError;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolHandler;
use codex_memories_mcp::LocalMemoriesBackend;
use codex_memories_mcp::backend::DEFAULT_LIST_MAX_RESULTS;
use codex_memories_mcp::backend::ListMemoriesRequest;
use codex_memories_mcp::backend::MAX_LIST_RESULTS;
use codex_memories_mcp::backend::MemoriesBackend;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde::Deserialize;
use serde_json::Value;

use super::backend_error;
use super::clamp_max_results;
use super::ensure_root;
use super::parse_arguments;
use super::serialize_output;

const TOOL_NAME: &str = "list_memories";

#[derive(Debug)]
pub(super) struct ListMemoriesTool {
    backend: LocalMemoriesBackend,
}

impl ListMemoriesTool {
    pub(super) fn new(backend: LocalMemoriesBackend) -> Self {
        Self { backend }
    }

    pub(super) fn contribution(self: &Arc<Self>) -> ToolContribution {
        let handler: Arc<dyn ToolHandler> = self.clone();
        ToolContribution::new(tool_spec(), handler).allow_parallel_calls()
    }
}

impl ToolHandler for ListMemoriesTool {
    fn handle<'a>(
        &'a self,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolCallError>> + Send + 'a>> {
        Box::pin(async move {
            let args: ListArgs = parse_arguments(arguments)?;
            ensure_root(&self.backend).await?;
            let response = self
                .backend
                .list(ListMemoriesRequest {
                    path: args.path,
                    cursor: args.cursor,
                    max_results: clamp_max_results(
                        args.max_results,
                        DEFAULT_LIST_MAX_RESULTS,
                        MAX_LIST_RESULTS,
                    ),
                })
                .await
                .map_err(backend_error)?;
            serialize_output(response)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    path: Option<String>,
    cursor: Option<String>,
    max_results: Option<usize>,
}

fn tool_spec() -> ResponsesApiTool {
    let properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Optional relative path to list inside the Codex memories store.".to_string(),
            )),
        ),
        (
            "cursor".to_string(),
            JsonSchema::string(Some(
                "Optional cursor returned by a previous list_memories call.".to_string(),
            )),
        ),
        (
            "max_results".to_string(),
            JsonSchema::integer(Some(
                "Optional maximum number of entries to return.".to_string(),
            )),
        ),
    ]);

    ResponsesApiTool {
        name: TOOL_NAME.to_string(),
        description:
            "List immediate files and directories under a path in the Codex memories store."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    }
}
