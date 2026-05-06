use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Identifies a callable tool, preserving the namespace split when the model
/// provides one.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ToolName {
    pub name: String,
    pub namespace: Option<String>,
}

impl ToolName {
    pub fn new(namespace: Option<String>, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace,
        }
    }

    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: None,
        }
    }

    pub fn namespaced(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: Some(namespace.into()),
        }
    }

    pub fn display(&self) -> String {
        match &self.namespace {
            Some(namespace) => flatten_namespaced_tool_name(namespace, &self.name),
            None => self.name.clone(),
        }
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.namespace {
            Some(namespace) => f.write_str(&flatten_namespaced_tool_name(namespace, &self.name)),
            None => f.write_str(&self.name),
        }
    }
}

fn flatten_namespaced_tool_name(namespace: &str, name: &str) -> String {
    if namespace.ends_with("__") || name.starts_with('_') {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}__{name}")
    }
}

impl From<String> for ToolName {
    fn from(name: String) -> Self {
        Self::plain(name)
    }
}

impl From<&str> for ToolName {
    fn from(name: &str) -> Self {
        Self::plain(name)
    }
}

#[cfg(test)]
mod tests {
    use super::ToolName;

    #[test]
    fn display_flattens_namespaced_tools_with_double_underscore_separator() {
        assert_eq!(ToolName::namespaced("foo_", "bar").display(), "foo___bar");
    }

    #[test]
    fn display_preserves_legacy_flattened_namespaced_tools() {
        assert_eq!(
            ToolName::namespaced("mcp__rmcp__", "echo").display(),
            "mcp__rmcp__echo"
        );
        assert_eq!(
            ToolName::namespaced("mcp__codex_apps__calendar", "_create_event").display(),
            "mcp__codex_apps__calendar_create_event"
        );
    }
}
