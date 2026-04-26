use std::collections::HashMap;

use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigLayerStackOrdering;
use codex_config::HookConfig;
use codex_config::HookConfigSource;
use codex_config::HookEventsToml;
use codex_protocol::protocol::HookEventName;

#[derive(Default)]
pub(crate) struct HookConfigRules {
    plugin: HashMap<(String, String), bool>,
}

impl HookConfigRules {
    pub(crate) fn from_stack(
        config_layer_stack: &ConfigLayerStack,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut rules = Self::default();
        for layer in config_layer_stack.get_layers(
            ConfigLayerStackOrdering::LowestPrecedenceFirst,
            /*include_disabled*/ true,
        ) {
            if !matches!(
                layer.name,
                ConfigLayerSource::User { .. } | ConfigLayerSource::SessionFlags
            ) {
                continue;
            }

            let Some(hooks_value) = layer.config.get("hooks") else {
                continue;
            };
            let hooks: HookEventsToml = match hooks_value.clone().try_into() {
                Ok(hooks) => hooks,
                Err(err) => {
                    warnings.push(format!("failed to parse TOML hooks config: {err}"));
                    continue;
                }
            };

            for entry in hooks.config {
                rules.append(entry, warnings);
            }
        }

        rules
    }

    pub(crate) fn enabled_for_plugin_hook(&self, plugin_id: &str, key: &str) -> bool {
        self.plugin
            .get(&(plugin_id.to_string(), key.to_string()))
            .copied()
            .unwrap_or({
                // TODO(abhinav): Default-enabled plugin hooks are temporary until hook trust is added.
                true
            })
    }

    fn append(&mut self, entry: HookConfig, warnings: &mut Vec<String>) {
        match entry.source {
            HookConfigSource::Plugin => {
                let Some(plugin_id) = entry.plugin_id else {
                    warnings.push(
                        "ignoring plugin hooks.config entry without a plugin_id selector"
                            .to_string(),
                    );
                    return;
                };
                if plugin_id.trim().is_empty() {
                    warnings.push(
                        "ignoring plugin hooks.config entry with empty plugin_id".to_string(),
                    );
                    return;
                }
                if entry.key.trim().is_empty() {
                    warnings.push("ignoring hooks.config entry with empty key".to_string());
                    return;
                }
                self.plugin.insert((plugin_id, entry.key), entry.enabled);
            }
        }
    }
}

pub(crate) fn hook_config_key(
    source_relative_path: &str,
    event_name: HookEventName,
    group_index: usize,
    handler_index: usize,
) -> String {
    format!(
        "{}:{}:{}:{}",
        source_relative_path,
        hook_event_name_config_label(event_name),
        group_index,
        handler_index
    )
}

fn hook_event_name_config_label(event_name: HookEventName) -> &'static str {
    match event_name {
        HookEventName::PreToolUse => "PreToolUse",
        HookEventName::PermissionRequest => "PermissionRequest",
        HookEventName::PostToolUse => "PostToolUse",
        HookEventName::SessionStart => "SessionStart",
        HookEventName::UserPromptSubmit => "UserPromptSubmit",
        HookEventName::Stop => "Stop",
    }
}
