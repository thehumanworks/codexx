use std::collections::BTreeMap;
use std::collections::HashSet;

use codex_protocol::models::ResponseItem;
use codex_tools::ToolName;

const MCP_TOOL_NAME_DELIMITER: &str = "__";

pub(crate) fn join_tool_name(tool_name: &ToolName) -> String {
    match tool_name.namespace.as_deref() {
        Some(namespace) => {
            let namespace = namespace.trim_end_matches('_');
            let name = tool_name.name.trim_start_matches('_');
            format!("{namespace}{MCP_TOOL_NAME_DELIMITER}{name}")
        }
        None => tool_name.name.clone(),
    }
}

pub(crate) fn collect_unavailable_called_tools(
    input: &[ResponseItem],
    exposed_tool_names: &HashSet<ToolName>,
) -> Vec<ToolName> {
    let mut unavailable_tools = BTreeMap::new();
    let exposed_joined_names = exposed_tool_names
        .iter()
        .map(join_tool_name)
        .collect::<HashSet<_>>();

    for item in input {
        let ResponseItem::FunctionCall {
            name, namespace, ..
        } = item
        else {
            continue;
        };
        if !should_collect_unavailable_tool(name, namespace.as_deref()) {
            continue;
        }

        let tool_name = match namespace {
            Some(namespace) => ToolName::namespaced(namespace.clone(), name.clone()),
            None => ToolName::plain(name.clone()),
        };
        let joined_name = join_tool_name(&tool_name);
        if exposed_joined_names.contains(&joined_name) {
            continue;
        }

        unavailable_tools
            .entry(joined_name)
            .or_insert_with(|| tool_name);
    }

    unavailable_tools.into_values().collect()
}

fn should_collect_unavailable_tool(name: &str, namespace: Option<&str>) -> bool {
    namespace.is_some_and(|namespace| namespace.starts_with("mcp__")) || name.starts_with("mcp__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn function_call(name: &str, namespace: Option<&str>) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: namespace.map(str::to_string),
            arguments: "{}".to_string(),
            call_id: format!("call-{name}"),
        }
    }

    #[test]
    fn collect_unavailable_called_tools_detects_mcp_function_calls() {
        let input = vec![
            function_call("shell", /*namespace*/ None),
            function_call("mcp__server__lookup", /*namespace*/ None),
            function_call("_create_event", Some("mcp__codex_apps__calendar")),
        ];

        let tools = collect_unavailable_called_tools(&input, &HashSet::new());

        assert_eq!(
            tools,
            vec![
                ToolName::namespaced("mcp__codex_apps__calendar", "_create_event"),
                ToolName::plain("mcp__server__lookup"),
            ]
        );
    }

    #[test]
    fn collect_unavailable_called_tools_skips_currently_available_tools() {
        let exposed_tool_names = HashSet::from([
            ToolName::plain("mcp__server__lookup"),
            ToolName::plain("mcp__server__search"),
        ]);
        let input = vec![
            function_call("mcp__server__lookup", /*namespace*/ None),
            function_call("mcp__server__search", /*namespace*/ None),
            function_call("mcp__server__missing", /*namespace*/ None),
        ];

        let tools = collect_unavailable_called_tools(&input, &exposed_tool_names);

        assert_eq!(tools, vec![ToolName::plain("mcp__server__missing")]);
    }

    #[test]
    fn collect_unavailable_called_tools_matches_exposed_joined_names() {
        let exposed_tool_names = HashSet::from([ToolName::namespaced("mcp__server", "lookup")]);
        let input = vec![function_call(
            "mcp__server__lookup",
            /*namespace*/ None,
        )];

        let tools = collect_unavailable_called_tools(&input, &exposed_tool_names);

        assert_eq!(tools, Vec::new());
    }

    #[test]
    fn collect_unavailable_called_tools_dedupes_by_joined_name() {
        let input = vec![
            function_call("lookup", Some("mcp__server")),
            function_call("mcp__server__lookup", /*namespace*/ None),
        ];

        let tools = collect_unavailable_called_tools(&input, &HashSet::new());

        assert_eq!(tools, vec![ToolName::namespaced("mcp__server", "lookup")]);
    }
}
