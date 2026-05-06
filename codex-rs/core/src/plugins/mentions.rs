use std::collections::HashMap;
use std::collections::HashSet;

use codex_connectors::metadata::connector_mention_slug;
use codex_protocol::user_input::UserInput;

use crate::connectors;
use crate::injection::ToolMentionKind;
use crate::injection::app_id_from_path;
use crate::injection::extract_tool_mentions_with_sigil;
use crate::injection::plugin_config_name_from_path;
use crate::injection::tool_kind_for_path;
use crate::mention_syntax::PLUGIN_TEXT_MENTION_SIGIL;
use crate::mention_syntax::TOOL_MENTION_SIGIL;

use super::PluginCapabilitySummary;

const COMPUTER_USE_PLUGIN_CONFIG_NAME: &str = "computer-use@openai-bundled";

pub(crate) struct CollectedToolMentions {
    pub(crate) plain_names: HashSet<String>,
    pub(crate) paths: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExplicitPluginMention {
    pub(crate) plugin: PluginCapabilitySummary,
    pub(crate) has_computer_use_native_fallback: bool,
}

#[derive(Clone, Debug)]
struct StructuredPluginMention {
    config_name: String,
    has_computer_use_native_fallback: bool,
}

pub(crate) fn collect_tool_mentions_from_messages(messages: &[String]) -> CollectedToolMentions {
    collect_tool_mentions_from_messages_with_sigil(messages, TOOL_MENTION_SIGIL)
}

fn collect_tool_mentions_from_messages_with_sigil(
    messages: &[String],
    sigil: char,
) -> CollectedToolMentions {
    let mut plain_names = HashSet::new();
    let mut paths = HashSet::new();
    for message in messages {
        let mentions = extract_tool_mentions_with_sigil(message, sigil);
        plain_names.extend(mentions.plain_names().map(str::to_string));
        paths.extend(mentions.paths().map(str::to_string));
    }
    CollectedToolMentions { plain_names, paths }
}

pub(crate) fn collect_explicit_app_ids(input: &[UserInput]) -> HashSet<String> {
    let messages = input
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<String>>();

    input
        .iter()
        .filter_map(|item| match item {
            UserInput::Mention { path, .. } => Some(path.clone()),
            _ => None,
        })
        .chain(collect_tool_mentions_from_messages(&messages).paths)
        .filter(|path| tool_kind_for_path(path.as_str()) == ToolMentionKind::App)
        .filter_map(|path| app_id_from_path(path.as_str()).map(str::to_string))
        .collect()
}

/// Collect explicit structured or linked `plugin://...` mentions.
pub(crate) fn collect_explicit_plugin_mentions(
    input: &[UserInput],
    plugins: &[PluginCapabilitySummary],
) -> Vec<ExplicitPluginMention> {
    if plugins.is_empty() {
        return Vec::new();
    }

    let messages = input
        .iter()
        .filter_map(|item| match item {
            UserInput::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<String>>();

    let structured_mentions = input
        .iter()
        .filter_map(|item| match item {
            UserInput::Mention {
                path,
                computer_use_native_app_bundle_id,
                ..
            } => Some((path.clone(), computer_use_native_app_bundle_id.clone())),
            _ => None,
        })
        .filter(|(path, _)| tool_kind_for_path(path.as_str()) == ToolMentionKind::Plugin)
        .filter_map(|(path, native_app_bundle_id)| {
            plugin_config_name_from_path(path.as_str()).map(|config_name| StructuredPluginMention {
                config_name: config_name.to_string(),
                has_computer_use_native_fallback: native_app_bundle_id
                    .as_deref()
                    .is_some_and(|bundle_id| !bundle_id.trim().is_empty()),
            })
        })
        .collect::<Vec<_>>();

    let linked_config_names =
        collect_tool_mentions_from_messages_with_sigil(&messages, PLUGIN_TEXT_MENTION_SIGIL)
            .paths
            .into_iter()
            .filter(|path| tool_kind_for_path(path.as_str()) == ToolMentionKind::Plugin)
            .filter_map(|path| plugin_config_name_from_path(path.as_str()).map(str::to_string))
            .collect::<HashSet<_>>();

    let mentioned_config_names = structured_mentions
        .iter()
        .map(|mention| mention.config_name.clone())
        .chain(linked_config_names)
        .collect::<HashSet<_>>();

    if mentioned_config_names.is_empty() {
        return Vec::new();
    }

    let mut mentioned_plugins = plugins
        .iter()
        .filter(|plugin| mentioned_config_names.contains(plugin.config_name.as_str()))
        .map(|plugin| {
            let has_computer_use_native_fallback = structured_mentions.iter().any(|mention| {
                mention.config_name == plugin.config_name
                    && mention.has_computer_use_native_fallback
            });
            ExplicitPluginMention {
                plugin: plugin.clone(),
                has_computer_use_native_fallback,
            }
        })
        .collect::<Vec<_>>();

    if mentioned_plugins
        .iter()
        .any(|mention| mention.has_computer_use_native_fallback)
        && !mentioned_plugins
            .iter()
            .any(|mention| mention.plugin.config_name.as_str() == COMPUTER_USE_PLUGIN_CONFIG_NAME)
        && let Some(computer_use_plugin) = plugins
            .iter()
            .find(|plugin| plugin.config_name.as_str() == COMPUTER_USE_PLUGIN_CONFIG_NAME)
    {
        mentioned_plugins.push(ExplicitPluginMention {
            plugin: computer_use_plugin.clone(),
            has_computer_use_native_fallback: false,
        });
    }

    mentioned_plugins
}

pub(crate) use crate::build_skill_name_counts;

pub(crate) fn build_connector_slug_counts(
    connectors: &[connectors::AppInfo],
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for connector in connectors {
        let slug = connector_mention_slug(connector);
        *counts.entry(slug).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
#[path = "mentions_tests.rs"]
mod tests;
