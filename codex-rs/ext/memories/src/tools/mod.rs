mod list;
mod read;
mod search;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use codex_extension_api::ToolCallError;
use codex_extension_api::ToolContribution;
use codex_memories_mcp::LocalMemoriesBackend;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use self::list::ListMemoriesTool;
use self::read::ReadMemoryTool;
use self::search::SearchMemoriesTool;

#[derive(Debug)]
pub(crate) struct MemoriesTools {
    list: Arc<ListMemoriesTool>,
    read: Arc<ReadMemoryTool>,
    search: Arc<SearchMemoriesTool>,
}

impl MemoriesTools {
    pub(crate) fn new(memories_root: impl Into<PathBuf>) -> Self {
        let backend = LocalMemoriesBackend::from_memory_root(memories_root);
        Self {
            list: Arc::new(ListMemoriesTool::new(backend.clone())),
            read: Arc::new(ReadMemoryTool::new(backend.clone())),
            search: Arc::new(SearchMemoriesTool::new(backend)),
        }
    }

    pub(crate) fn contributions(&self) -> Vec<ToolContribution> {
        vec![
            self.list.contribution(),
            self.read.contribution(),
            self.search.contribution(),
        ]
    }
}

async fn ensure_root(backend: &LocalMemoriesBackend) -> Result<(), ToolCallError> {
    tokio::fs::create_dir_all(backend.root())
        .await
        .map_err(|err| {
            ToolCallError::new(format!(
                "failed to create memories root at {}: {err}",
                backend.root().display()
            ))
        })
}

fn parse_arguments<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolCallError> {
    serde_json::from_value(arguments)
        .map_err(|err| ToolCallError::new(format!("invalid arguments: {err}")))
}

fn serialize_output<T: Serialize>(output: T) -> Result<Value, ToolCallError> {
    serde_json::to_value(output)
        .map_err(|err| ToolCallError::new(format!("failed to serialize output: {err}")))
}

fn backend_error(err: impl Display) -> ToolCallError {
    ToolCallError::new(err.to_string())
}

fn clamp_max_results(requested: Option<usize>, default: usize, max: usize) -> usize {
    requested.unwrap_or(default).clamp(1, max)
}
