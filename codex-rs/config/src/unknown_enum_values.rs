use crate::config_toml::RealtimeTransport;
use crate::config_toml::RealtimeVoice;
use crate::config_toml::RealtimeWsMode;
use crate::config_toml::RealtimeWsVersion;
use crate::config_toml::ThreadStoreToml;
use crate::types::ApprovalsReviewer;
use crate::types::AuthCredentialsStoreMode;
use crate::types::HistoryPersistence;
use crate::types::OAuthCredentialsStoreMode;
use crate::types::Personality;
use crate::types::ServiceTier;
use crate::types::UriBasedFileOpener;
use crate::types::WebSearchMode;
use crate::types::WindowsSandboxModeToml;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::TrustLevel;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use serde::Deserialize;
use toml::Value as TomlValue;

/// Removes unrecognized string values from enum-typed config fields.
///
/// This keeps older clients from failing to load a config written by a newer
/// client that knows about a newly added enum variant. The field is treated as
/// unset, so the normal default/resolution path applies. Non-string shape
/// errors are left intact and still fail during typed deserialization.
pub fn sanitize_unknown_enum_values(root: &mut TomlValue) -> Vec<String> {
    let mut warnings = Vec::new();

    sanitize_enum::<AskForApproval>(root, &["approval_policy"], &mut warnings);
    sanitize_enum::<ApprovalsReviewer>(root, &["approvals_reviewer"], &mut warnings);
    sanitize_enum::<SandboxMode>(root, &["sandbox_mode"], &mut warnings);
    sanitize_enum::<ForcedLoginMethod>(root, &["forced_login_method"], &mut warnings);
    sanitize_enum::<AuthCredentialsStoreMode>(root, &["cli_auth_credentials_store"], &mut warnings);
    sanitize_enum::<OAuthCredentialsStoreMode>(
        root,
        &["mcp_oauth_credentials_store"],
        &mut warnings,
    );
    sanitize_enum::<UriBasedFileOpener>(root, &["file_opener"], &mut warnings);
    sanitize_enum::<ReasoningEffort>(root, &["model_reasoning_effort"], &mut warnings);
    sanitize_enum::<ReasoningEffort>(root, &["plan_mode_reasoning_effort"], &mut warnings);
    sanitize_enum::<ReasoningSummary>(root, &["model_reasoning_summary"], &mut warnings);
    sanitize_enum::<Verbosity>(root, &["model_verbosity"], &mut warnings);
    sanitize_enum::<Personality>(root, &["personality"], &mut warnings);
    sanitize_enum::<ServiceTier>(root, &["service_tier"], &mut warnings);
    sanitize_enum::<WebSearchMode>(root, &["web_search"], &mut warnings);
    sanitize_enum::<HistoryPersistence>(root, &["history", "persistence"], &mut warnings);
    sanitize_enum::<WindowsSandboxModeToml>(root, &["windows", "sandbox"], &mut warnings);
    sanitize_enum::<RealtimeWsVersion>(root, &["realtime", "version"], &mut warnings);
    sanitize_enum::<RealtimeWsMode>(root, &["realtime", "type"], &mut warnings);
    sanitize_enum::<RealtimeTransport>(root, &["realtime", "transport"], &mut warnings);
    sanitize_enum::<RealtimeVoice>(root, &["realtime", "voice"], &mut warnings);
    sanitize_tagged_enum::<ThreadStoreToml>(
        root,
        &["experimental_thread_store"],
        "type",
        &mut warnings,
    );
    sanitize_enum::<WebSearchContextSize>(
        root,
        &["tools", "web_search", "context_size"],
        &mut warnings,
    );

    sanitize_table_entries(
        root,
        &["profiles"],
        |value, prefix, warnings| {
            sanitize_enum_with_prefix::<ServiceTier>(value, prefix, &["service_tier"], warnings);
            sanitize_enum_with_prefix::<AskForApproval>(
                value,
                prefix,
                &["approval_policy"],
                warnings,
            );
            sanitize_enum_with_prefix::<ApprovalsReviewer>(
                value,
                prefix,
                &["approvals_reviewer"],
                warnings,
            );
            sanitize_enum_with_prefix::<SandboxMode>(value, prefix, &["sandbox_mode"], warnings);
            sanitize_enum_with_prefix::<ReasoningEffort>(
                value,
                prefix,
                &["model_reasoning_effort"],
                warnings,
            );
            sanitize_enum_with_prefix::<ReasoningEffort>(
                value,
                prefix,
                &["plan_mode_reasoning_effort"],
                warnings,
            );
            sanitize_enum_with_prefix::<ReasoningSummary>(
                value,
                prefix,
                &["model_reasoning_summary"],
                warnings,
            );
            sanitize_enum_with_prefix::<Verbosity>(value, prefix, &["model_verbosity"], warnings);
            sanitize_enum_with_prefix::<Personality>(value, prefix, &["personality"], warnings);
            sanitize_enum_with_prefix::<WebSearchMode>(value, prefix, &["web_search"], warnings);
            sanitize_enum_with_prefix::<WindowsSandboxModeToml>(
                value,
                prefix,
                &["windows", "sandbox"],
                warnings,
            );
            sanitize_enum_with_prefix::<WebSearchContextSize>(
                value,
                prefix,
                &["tools", "web_search", "context_size"],
                warnings,
            );
        },
        &mut warnings,
    );

    sanitize_table_entries(
        root,
        &["projects"],
        |value, prefix, warnings| {
            sanitize_enum_with_prefix::<TrustLevel>(value, prefix, &["trust_level"], warnings);
        },
        &mut warnings,
    );

    warnings
}

fn sanitize_enum<T>(root: &mut TomlValue, path: &[&str], warnings: &mut Vec<String>)
where
    T: for<'de> Deserialize<'de>,
{
    sanitize_enum_with_prefix::<T>(root, &[], path, warnings);
}

fn sanitize_enum_with_prefix<T>(
    root: &mut TomlValue,
    prefix: &[String],
    path: &[&str],
    warnings: &mut Vec<String>,
) where
    T: for<'de> Deserialize<'de>,
{
    let Some(value) = value_at_path(root, path) else {
        return;
    };
    let Some(raw_value) = value.as_str() else {
        return;
    };
    let parsed: Result<T, _> = value.clone().try_into();
    if parsed.is_ok() {
        return;
    }

    let field_path = display_path(prefix, path);
    warnings.push(format!(
        "Ignoring unrecognized config value `{raw_value}` for `{field_path}`; using the default for this setting."
    ));
    tracing::warn!(
        field = field_path,
        value = raw_value,
        "ignoring unrecognized config enum value"
    );
    remove_value_at_path(root, path);
}

fn sanitize_tagged_enum<T>(
    root: &mut TomlValue,
    path: &[&str],
    tag: &str,
    warnings: &mut Vec<String>,
) where
    T: for<'de> Deserialize<'de>,
{
    let Some(value) = value_at_path(root, path) else {
        return;
    };
    let Some(table) = value.as_table() else {
        return;
    };
    let Some(raw_value) = table.get(tag).and_then(TomlValue::as_str) else {
        return;
    };
    let parsed: Result<T, _> = value.clone().try_into();
    if parsed.is_ok() {
        return;
    }

    let field_path = display_path(&[], path);
    warnings.push(format!(
        "Ignoring unrecognized config value `{raw_value}` for `{field_path}.{tag}`; using the default for this setting."
    ));
    tracing::warn!(
        field = format!("{field_path}.{tag}"),
        value = raw_value,
        "ignoring unrecognized config enum value"
    );
    remove_value_at_path(root, path);
}

fn sanitize_table_entries(
    root: &mut TomlValue,
    path: &[&str],
    mut sanitize_entry: impl FnMut(&mut TomlValue, &[String], &mut Vec<String>),
    warnings: &mut Vec<String>,
) {
    let Some(TomlValue::Table(table)) = value_at_path_mut(root, path) else {
        return;
    };

    for (key, value) in table {
        let mut prefix = path
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>();
        prefix.push(key.clone());
        sanitize_entry(value, &prefix, warnings);
    }
}

fn value_at_path_mut<'a>(root: &'a mut TomlValue, path: &[&str]) -> Option<&'a mut TomlValue> {
    let mut value = root;
    for part in path {
        value = value.as_table_mut()?.get_mut(*part)?;
    }
    Some(value)
}

fn value_at_path<'a>(root: &'a TomlValue, path: &[&str]) -> Option<&'a TomlValue> {
    let mut value = root;
    for part in path {
        value = value.as_table()?.get(*part)?;
    }
    Some(value)
}

fn remove_value_at_path(root: &mut TomlValue, path: &[&str]) {
    let Some((last, parent_path)) = path.split_last() else {
        return;
    };
    let Some(parent) = value_at_path_mut(root, parent_path).and_then(TomlValue::as_table_mut)
    else {
        return;
    };
    parent.remove(*last);
}

fn display_path(prefix: &[String], path: &[&str]) -> String {
    prefix
        .iter()
        .map(String::as_str)
        .chain(path.iter().copied())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_toml::ConfigToml;
    use pretty_assertions::assert_eq;

    #[test]
    fn unknown_config_enum_values_are_removed_with_warnings() {
        let mut value: TomlValue = toml::from_str(
            r#"
service_tier = "ultrafast"
sandbox_mode = "workspace-write"

[profiles.future]
model_reasoning_effort = "huge"
model_verbosity = "high"

[projects."/tmp/project"]
trust_level = "very-trusted"

[tools.web_search]
context_size = "massive"
"#,
        )
        .expect("config should parse as TOML");

        let warnings = sanitize_unknown_enum_values(&mut value);
        let expected: TomlValue = toml::from_str(
            r#"
sandbox_mode = "workspace-write"

[profiles.future]
model_verbosity = "high"

[projects."/tmp/project"]

[tools.web_search]
"#,
        )
        .expect("expected TOML should parse");

        assert_eq!(
            (
                expected,
                vec![
                    "Ignoring unrecognized config value `ultrafast` for `service_tier`; using the default for this setting.".to_string(),
                    "Ignoring unrecognized config value `huge` for `profiles.future.model_reasoning_effort`; using the default for this setting.".to_string(),
                    "Ignoring unrecognized config value `very-trusted` for `projects./tmp/project.trust_level`; using the default for this setting.".to_string(),
                    "Ignoring unrecognized config value `massive` for `tools.web_search.context_size`; using the default for this setting.".to_string(),
                ],
            ),
            (value, warnings)
        );
    }

    #[test]
    fn unknown_config_enum_values_allow_config_toml_deserialization() {
        let mut value: TomlValue = toml::from_str(
            r#"
service_tier = "ultrafast"
model_reasoning_summary = "verbose"
approval_policy = "on-request"
"#,
        )
        .expect("config should parse as TOML");

        let warnings = sanitize_unknown_enum_values(&mut value);
        let config: ConfigToml = value.try_into().expect("config should deserialize");

        assert_eq!(
            (
                None,
                None,
                Some(AskForApproval::OnRequest),
                vec![
                    "Ignoring unrecognized config value `ultrafast` for `service_tier`; using the default for this setting.".to_string(),
                    "Ignoring unrecognized config value `verbose` for `model_reasoning_summary`; using the default for this setting.".to_string(),
                ],
            ),
            (
                config.service_tier,
                config.model_reasoning_summary,
                config.approval_policy,
                warnings,
            )
        );
    }
}
