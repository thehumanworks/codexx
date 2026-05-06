#[cfg(test)]
use crate::context::AvailablePluginsInstructions;
#[cfg(test)]
use crate::context::ContextualUserFragment;
use crate::plugins::PluginCapabilitySummary;

#[cfg(test)]
pub(crate) fn render_plugins_section(plugins: &[PluginCapabilitySummary]) -> Option<String> {
    AvailablePluginsInstructions::from_plugins(plugins).map(|instructions| instructions.render())
}

pub(crate) fn render_explicit_plugin_instructions(
    plugin: &PluginCapabilitySummary,
    available_mcp_servers: &[String],
    available_apps: &[String],
    has_computer_use_native_fallback: bool,
) -> Option<String> {
    let mut lines = vec![format!(
        "Capabilities from the `{}` plugin:",
        plugin.display_name
    )];

    if plugin.has_skills {
        lines.push(format!(
            "- Skills from this plugin are prefixed with `{}:`.",
            plugin.display_name
        ));
    }

    if !available_mcp_servers.is_empty() {
        lines.push(format!(
            "- MCP servers from this plugin available in this session: {}.",
            available_mcp_servers
                .iter()
                .map(|server| format!("`{server}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if !available_apps.is_empty() {
        lines.push(format!(
            "- Apps from this plugin available in this session: {}.",
            available_apps
                .iter()
                .map(|app| format!("`{app}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if has_computer_use_native_fallback {
        lines.push(
            "- This plugin also corresponds to a native desktop app available through Computer Use. Prefer plugin-associated capabilities first; if they are unavailable, insufficient, or fail, use the Computer Use tool surface for the native app fallback. If Computer Use tools are not already visible, use `tool_search` for `computer use` to discover them."
                .to_string(),
        );
    }

    if lines.len() == 1 {
        return None;
    }

    lines.push("Use these plugin-associated capabilities to help solve the task.".to_string());

    Some(lines.join("\n"))
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
