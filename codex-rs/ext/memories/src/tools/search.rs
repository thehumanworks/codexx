use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use codex_extension_api::ToolCallError;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolHandler;
use codex_memories_mcp::LocalMemoriesBackend;
use codex_memories_mcp::backend::DEFAULT_SEARCH_MAX_RESULTS;
use codex_memories_mcp::backend::MAX_SEARCH_RESULTS;
use codex_memories_mcp::backend::MemoriesBackend;
use codex_memories_mcp::backend::SearchMatchMode;
use codex_memories_mcp::backend::SearchMemoriesRequest;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use super::backend_error;
use super::clamp_max_results;
use super::ensure_root;
use super::parse_arguments;
use super::serialize_output;

const TOOL_NAME: &str = "search_memories";

#[derive(Debug)]
pub(super) struct SearchMemoriesTool {
    backend: LocalMemoriesBackend,
}

impl SearchMemoriesTool {
    pub(super) fn new(backend: LocalMemoriesBackend) -> Self {
        Self { backend }
    }

    pub(super) fn contribution(self: &Arc<Self>) -> ToolContribution {
        let handler: Arc<dyn ToolHandler> = self.clone();
        ToolContribution::new(tool_spec(), handler).allow_parallel_calls()
    }
}

impl ToolHandler for SearchMemoriesTool {
    fn handle<'a>(
        &'a self,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolCallError>> + Send + 'a>> {
        Box::pin(async move {
            let args: SearchArgs = parse_arguments(arguments)?;
            ensure_root(&self.backend).await?;
            let response = self
                .backend
                .search(args.into_request())
                .await
                .map_err(backend_error)?;
            serialize_output(response)
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    queries: Vec<String>,
    match_mode: Option<SearchMatchMode>,
    path: Option<String>,
    cursor: Option<String>,
    context_lines: Option<usize>,
    case_sensitive: Option<bool>,
    normalized: Option<bool>,
    max_results: Option<usize>,
}

impl SearchArgs {
    fn into_request(self) -> SearchMemoriesRequest {
        SearchMemoriesRequest {
            queries: self.queries,
            match_mode: self.match_mode.unwrap_or(SearchMatchMode::Any),
            path: self.path,
            cursor: self.cursor,
            context_lines: self.context_lines.unwrap_or(0),
            case_sensitive: self.case_sensitive.unwrap_or(true),
            normalized: self.normalized.unwrap_or(false),
            max_results: clamp_max_results(
                self.max_results,
                DEFAULT_SEARCH_MAX_RESULTS,
                MAX_SEARCH_RESULTS,
            ),
        }
    }
}

fn tool_spec() -> ResponsesApiTool {
    let properties = BTreeMap::from([
        (
            "queries".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some("Substrings to search for.".to_string()),
            ),
        ),
        ("match_mode".to_string(), search_match_mode_schema()),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Optional relative path to search inside the Codex memories store.".to_string(),
            )),
        ),
        (
            "cursor".to_string(),
            JsonSchema::string(Some(
                "Optional cursor returned by a previous search_memories call.".to_string(),
            )),
        ),
        (
            "context_lines".to_string(),
            JsonSchema::integer(Some(
                "Optional number of context lines to include around each match.".to_string(),
            )),
        ),
        (
            "case_sensitive".to_string(),
            JsonSchema::boolean(Some(
                "Whether matching should be case-sensitive.".to_string(),
            )),
        ),
        (
            "normalized".to_string(),
            JsonSchema::boolean(Some(
                "Whether to normalize separators while matching.".to_string(),
            )),
        ),
        (
            "max_results".to_string(),
            JsonSchema::integer(Some(
                "Optional maximum number of matches to return.".to_string(),
            )),
        ),
    ]);

    ResponsesApiTool {
        name: TOOL_NAME.to_string(),
        description:
            "Search Codex memory files for substring matches, optionally normalizing separators or requiring all query substrings on the same line or within a line window."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["queries".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    }
}

fn search_match_mode_schema() -> JsonSchema {
    JsonSchema::any_of(
        vec![
            tagged_match_mode_schema("any", None),
            tagged_match_mode_schema("all_on_same_line", None),
            tagged_match_mode_schema(
                "all_within_lines",
                Some((
                    "line_count".to_string(),
                    JsonSchema::integer(Some(
                        "Positive number of lines in the matching window.".to_string(),
                    )),
                )),
            ),
        ],
        Some("How multiple queries must match.".to_string()),
    )
}

fn tagged_match_mode_schema(
    name: &str,
    extra_property: Option<(String, JsonSchema)>,
) -> JsonSchema {
    let mut properties = BTreeMap::from([(
        "type".to_string(),
        JsonSchema::string_enum(vec![json!(name)], None),
    )]);
    let mut required = vec!["type".to_string()];
    if let Some((name, schema)) = extra_property {
        required.push(name.clone());
        properties.insert(name, schema);
    }
    JsonSchema::object(properties, Some(required), Some(false.into()))
}
