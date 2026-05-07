use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::protocol::HookEventName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as SerdeError;
use serde_json::Value as JsonValue;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HooksFile {
    #[serde(default)]
    pub hooks: HookEventsToml,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HooksToml {
    #[serde(flatten)]
    pub events: HookEventsToml,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state: BTreeMap<String, HookStateToml>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookStateToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_hash: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HookEventsToml {
    #[serde(rename = "PreToolUse", default)]
    pub pre_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "PermissionRequest", default)]
    pub permission_request: Vec<MatcherGroup>,
    #[serde(rename = "PostToolUse", default)]
    pub post_tool_use: Vec<MatcherGroup>,
    #[serde(rename = "SessionStart", default)]
    pub session_start: Vec<MatcherGroup>,
    #[serde(rename = "UserPromptSubmit", default)]
    pub user_prompt_submit: Vec<MatcherGroup>,
    #[serde(rename = "Stop", default)]
    pub stop: Vec<MatcherGroup>,
}

impl HookEventsToml {
    pub fn is_empty(&self) -> bool {
        let Self {
            pre_tool_use,
            permission_request,
            post_tool_use,
            session_start,
            user_prompt_submit,
            stop,
        } = self;
        pre_tool_use.is_empty()
            && permission_request.is_empty()
            && post_tool_use.is_empty()
            && session_start.is_empty()
            && user_prompt_submit.is_empty()
            && stop.is_empty()
    }

    pub fn handler_count(&self) -> usize {
        let Self {
            pre_tool_use,
            permission_request,
            post_tool_use,
            session_start,
            user_prompt_submit,
            stop,
        } = self;
        [
            pre_tool_use,
            permission_request,
            post_tool_use,
            session_start,
            user_prompt_submit,
            stop,
        ]
        .into_iter()
        .flatten()
        .map(|group| group.hooks.len())
        .sum()
    }

    pub fn into_matcher_groups(self) -> [(HookEventName, Vec<MatcherGroup>); 6] {
        [
            (HookEventName::PreToolUse, self.pre_tool_use),
            (HookEventName::PermissionRequest, self.permission_request),
            (HookEventName::PostToolUse, self.post_tool_use),
            (HookEventName::SessionStart, self.session_start),
            (HookEventName::UserPromptSubmit, self.user_prompt_submit),
            (HookEventName::Stop, self.stop),
        ]
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MatcherGroup {
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default, deserialize_with = "deserialize_hook_handlers")]
    pub hooks: Vec<HookHandlerConfig>,
}

/// Deserialize hook handlers while dropping entries with unknown tagged variants.
///
/// The schema warning pass reports invalid `type` values before typed config
/// deserialization. Dropping only those entries keeps startup warnings
/// non-blocking without making unrelated hook shape errors silent.
fn deserialize_hook_handlers<'de, D>(deserializer: D) -> Result<Vec<HookHandlerConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<JsonValue>::deserialize(deserializer)?;
    let mut handlers = Vec::new();

    for value in values {
        let invalid_type = value.get("type").is_some_and(|handler_type| {
            !matches!(handler_type.as_str(), Some("command" | "prompt" | "agent"))
        });
        match serde_json::from_value(value) {
            Ok(handler) => handlers.push(handler),
            Err(_) if invalid_type => {}
            Err(err) => return Err(SerdeError::custom(err)),
        }
    }

    Ok(handlers)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum HookHandlerConfig {
    #[serde(rename = "command")]
    Command {
        command: String,
        #[serde(default, rename = "timeout")]
        timeout_sec: Option<u64>,
        #[serde(default)]
        r#async: bool,
        #[serde(default, rename = "statusMessage")]
        status_message: Option<String>,
    },
    #[serde(rename = "prompt")]
    Prompt {},
    #[serde(rename = "agent")]
    Agent {},
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedHooksRequirementsToml {
    pub managed_dir: Option<PathBuf>,
    pub windows_managed_dir: Option<PathBuf>,
    #[serde(flatten)]
    pub hooks: HookEventsToml,
}

impl ManagedHooksRequirementsToml {
    pub fn is_empty(&self) -> bool {
        let Self {
            managed_dir,
            windows_managed_dir,
            hooks,
        } = self;
        managed_dir.is_none() && windows_managed_dir.is_none() && hooks.is_empty()
    }

    pub fn handler_count(&self) -> usize {
        self.hooks.handler_count()
    }

    pub fn managed_dir_for_current_platform(&self) -> Option<&Path> {
        #[cfg(windows)]
        {
            self.windows_managed_dir.as_deref()
        }

        #[cfg(not(windows))]
        {
            self.managed_dir.as_deref()
        }
    }
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
