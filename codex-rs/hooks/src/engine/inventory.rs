use codex_config::ConfigLayerStack;
use codex_config::HookHandlerConfig;
use codex_plugin::PluginHookSource;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookHandlerType;
use codex_protocol::protocol::HookSource;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::config_rules::HookConfigRules;
use super::config_rules::hook_config_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookInventoryEntry {
    pub source: HookSource,
    pub plugin_id: Option<String>,
    pub key: String,
    pub event_name: HookEventName,
    pub matcher: Option<String>,
    pub handler_type: HookHandlerType,
    pub command: Option<String>,
    pub timeout_sec: Option<u64>,
    pub status_message: Option<String>,
    pub source_path: AbsolutePathBuf,
    pub source_relative_path: Option<String>,
    pub enabled: bool,
}

pub fn list_plugin_hooks(
    config_layer_stack: Option<&ConfigLayerStack>,
    plugin_hook_sources: &[PluginHookSource],
) -> Vec<HookInventoryEntry> {
    let mut warnings = Vec::new();
    let hook_config_rules = config_layer_stack
        .map(|config_layer_stack| HookConfigRules::from_stack(config_layer_stack, &mut warnings))
        .unwrap_or_default();
    let mut entries = Vec::new();

    for source in plugin_hook_sources {
        let plugin_id = source.plugin_id.as_key();
        for (event_name, groups) in source.hooks.clone().into_matcher_groups() {
            for (group_index, group) in groups.into_iter().enumerate() {
                for (handler_index, handler) in group.hooks.into_iter().enumerate() {
                    let key = hook_config_key(
                        &source.source_relative_path,
                        event_name,
                        group_index,
                        handler_index,
                    );
                    let enabled = hook_config_rules.enabled_for_plugin_hook(&plugin_id, &key);
                    let (handler_type, command, timeout_sec, status_message) =
                        hook_inventory_handler_fields(handler);
                    entries.push(HookInventoryEntry {
                        source: HookSource::Plugin,
                        plugin_id: Some(plugin_id.clone()),
                        key,
                        event_name,
                        matcher: group.matcher.clone(),
                        handler_type,
                        command,
                        timeout_sec,
                        status_message,
                        source_path: source.source_path.clone(),
                        source_relative_path: Some(source.source_relative_path.clone()),
                        enabled,
                    });
                }
            }
        }
    }

    entries
}

fn hook_inventory_handler_fields(
    handler: HookHandlerConfig,
) -> (HookHandlerType, Option<String>, Option<u64>, Option<String>) {
    match handler {
        HookHandlerConfig::Command {
            command,
            timeout_sec,
            r#async: _,
            status_message,
        } => (
            HookHandlerType::Command,
            Some(command),
            timeout_sec,
            status_message,
        ),
        HookHandlerConfig::Prompt {} => (HookHandlerType::Prompt, None, None, None),
        HookHandlerConfig::Agent {} => (HookHandlerType::Agent, None, None, None),
    }
}
