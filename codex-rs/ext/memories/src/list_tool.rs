// This file must be replaced by tools from `codex-memories-mcp` once the extracted tools land. This is just a vibe-coded copy/paster for now.

use std::borrow::Cow;
use std::future::Future;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use codex_extension_api::ToolCallError;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolHandler;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

const LIST_MEMORIES_TOOL_NAME: &str = "list_memories";
const DEFAULT_LIST_MAX_RESULTS: usize = 2_000;
const MAX_LIST_RESULTS: usize = 2_000;

#[derive(Debug)]
pub(super) struct ListMemoriesTool {
    memories_root: PathBuf,
}

impl ListMemoriesTool {
    pub(super) fn new(memories_root: impl Into<PathBuf>) -> Self {
        Self {
            memories_root: memories_root.into(),
        }
    }

    pub(super) fn contribution<C>(self: &Arc<Self>) -> ToolContribution<C>
    where
        C: Send + Sync + 'static,
    {
        let handler: Arc<dyn ToolHandler<C>> = self.clone();
        ToolContribution::new(create_list_memories_tool(), handler).allow_parallel_calls()
    }
}

impl<C> ToolHandler<C> for ListMemoriesTool
where
    C: Send + Sync,
{
    fn handle<'a>(
        &'a self,
        _context: &'a C,
        arguments: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolCallError>> + Send + 'a>> {
        Box::pin(async move {
            let args: ListMemoriesArgs = serde_json::from_value(arguments)
                .map_err(|err| ToolCallError::new(format!("invalid arguments: {err}")))?;
            tokio::fs::create_dir_all(&self.memories_root)
                .await
                .map_err(|err| {
                    ToolCallError::new(format!(
                        "failed to create memories root at {}: {err}",
                        self.memories_root.display()
                    ))
                })?;
            let response = list_memories(&self.memories_root, args)
                .await
                .map_err(|err| ToolCallError::new(err.to_string()))?;
            serde_json::to_value(response)
                .map_err(|err| ToolCallError::new(format!("failed to serialize output: {err}")))
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListMemoriesArgs {
    path: Option<String>,
    cursor: Option<String>,
    max_results: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ListMemoriesResponse {
    path: Option<String>,
    entries: Vec<MemoryEntry>,
    next_cursor: Option<String>,
    truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MemoryEntry {
    path: String,
    entry_type: MemoryEntryType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum MemoryEntryType {
    File,
    Directory,
}

#[derive(Debug, thiserror::Error)]
enum ListMemoriesError {
    #[error("path '{path}' {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("cursor '{cursor}' {reason}")]
    InvalidCursor { cursor: String, reason: String },
    #[error("path '{path}' was not found")]
    NotFound { path: String },
    #[error("I/O error while reading memories: {0}")]
    Io(#[from] std::io::Error),
}

async fn list_memories(
    memories_root: &Path,
    args: ListMemoriesArgs,
) -> Result<ListMemoriesResponse, ListMemoriesError> {
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_LIST_MAX_RESULTS)
        .min(MAX_LIST_RESULTS);
    let start = resolve_scoped_path(memories_root, args.path.as_deref()).await?;
    let start_index = match args.cursor.as_deref() {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| ListMemoriesError::InvalidCursor {
                cursor: cursor.to_string(),
                reason: "must be a non-negative integer".to_string(),
            })?,
        None => 0,
    };
    let Some(metadata) = metadata_or_none(&start).await? else {
        return Err(ListMemoriesError::NotFound {
            path: args.path.unwrap_or_default(),
        });
    };
    reject_symlink(&display_relative_path(memories_root, &start), &metadata)?;

    let mut entries = if metadata.is_file() {
        vec![MemoryEntry {
            path: display_relative_path(memories_root, &start),
            entry_type: MemoryEntryType::File,
        }]
    } else if metadata.is_dir() {
        let mut entries = Vec::new();
        for path in read_sorted_dir_paths(&start).await? {
            if is_hidden_path(&path) {
                continue;
            }
            let Some(metadata) = metadata_or_none(&path).await? else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }

            let entry_type = if metadata.is_dir() {
                MemoryEntryType::Directory
            } else if metadata.is_file() {
                MemoryEntryType::File
            } else {
                continue;
            };
            entries.push(MemoryEntry {
                path: display_relative_path(memories_root, &path),
                entry_type,
            });
        }
        entries
    } else {
        Vec::new()
    };
    if start_index > entries.len() {
        return Err(ListMemoriesError::InvalidCursor {
            cursor: start_index.to_string(),
            reason: "exceeds result count".to_string(),
        });
    }

    let end_index = start_index.saturating_add(max_results).min(entries.len());
    let next_cursor = (end_index < entries.len()).then(|| end_index.to_string());
    let truncated = next_cursor.is_some();
    Ok(ListMemoriesResponse {
        path: args.path,
        entries: entries.drain(start_index..end_index).collect(),
        next_cursor,
        truncated,
    })
}

async fn resolve_scoped_path(
    memories_root: &Path,
    relative_path: Option<&str>,
) -> Result<PathBuf, ListMemoriesError> {
    let Some(relative_path) = relative_path else {
        return Ok(memories_root.to_path_buf());
    };
    let relative = Path::new(relative_path);
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(ListMemoriesError::InvalidPath {
            path: relative_path.to_string(),
            reason: "must stay within the memories root".to_string(),
        });
    }
    if relative.components().any(is_hidden_component) {
        return Err(ListMemoriesError::NotFound {
            path: relative_path.to_string(),
        });
    }

    let components = relative.components().collect::<Vec<_>>();
    let mut scoped_path = memories_root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        scoped_path.push(component.as_os_str());

        let Some(metadata) = metadata_or_none(&scoped_path).await? else {
            for remaining_component in components.iter().skip(index + 1) {
                scoped_path.push(remaining_component.as_os_str());
            }
            return Ok(scoped_path);
        };

        reject_symlink(
            &display_relative_path(memories_root, &scoped_path),
            &metadata,
        )?;
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(ListMemoriesError::InvalidPath {
                path: relative_path.to_string(),
                reason: "traverses through a non-directory path component".to_string(),
            });
        }
    }

    Ok(scoped_path)
}

async fn metadata_or_none(path: &Path) -> Result<Option<std::fs::Metadata>, ListMemoriesError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn reject_symlink(
    relative_path: &str,
    metadata: &std::fs::Metadata,
) -> Result<(), ListMemoriesError> {
    if metadata.file_type().is_symlink() {
        return Err(ListMemoriesError::InvalidPath {
            path: relative_path.to_string(),
            reason: "must not be a symlink".to_string(),
        });
    }
    Ok(())
}

fn display_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map_or(Cow::Borrowed(path), Cow::Borrowed)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn is_hidden_component(component: Component<'_>) -> bool {
    matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('.'))
}

async fn read_sorted_dir_paths(path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut paths = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        paths.push(entry.path());
    }
    paths.sort();
    Ok(paths)
}

fn create_list_memories_tool() -> ResponsesApiTool {
    let properties = std::collections::BTreeMap::from([
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
        name: LIST_MEMORIES_TOOL_NAME.to_string(),
        description:
            "List immediate files and directories under a path in the Codex memories store."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, /*required*/ None, Some(false.into())),
        output_schema: None,
    }
}
