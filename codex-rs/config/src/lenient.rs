use crate::config_toml::ConfigToml;
use crate::config_toml::RealtimeTransport;
use crate::config_toml::RealtimeVoice;
use crate::config_toml::RealtimeWsMode;
use crate::config_toml::RealtimeWsVersion;
use crate::config_toml::ThreadStoreToml;
use crate::mcp_types::AppToolApproval;
use crate::types::AltScreenMode;
use crate::types::ApprovalsReviewer;
use crate::types::AuthCredentialsStoreMode;
use crate::types::HistoryPersistence;
use crate::types::MarketplaceSourceType;
use crate::types::NotificationCondition;
use crate::types::NotificationMethod;
use crate::types::Notifications;
use crate::types::OAuthCredentialsStoreMode;
use crate::types::SessionPickerViewMode;
use crate::types::UriBasedFileOpener;
use crate::types::WindowsSandboxModeToml;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use serde::de::DeserializeOwned;
use toml::Value as TomlValue;

const SHELL_ENVIRONMENT_POLICY: &str = "shell_environment_policy";
const CONTEXT_SIZE: &str = "context_size";
const DEFAULT_TOOLS_APPROVAL_MODE: &str = "default_tools_approval_mode";
const APPROVAL_MODE: &str = "approval_mode";

macro_rules! check_fields {
    ($value:expr, $path:expr, $warnings:expr, $( $key:expr => $ty:ty ),+ $(,)?) => {
        $(
            check_value::<$ty>($value, &child_path($path, $key), $warnings);
        )+
    };
}

/// Deserializes config while turning known invalid enum-valued settings into warnings.
///
/// The config structs use `serde_with::DefaultOnError` on their enum fields so
/// an invalid field can default instead of rejecting the whole file. This helper
/// runs the companion best-effort scan over the raw TOML first. When a known
/// enum-bearing leaf fails to parse as its real type, the leaf is removed from
/// the sanitized TOML and a startup warning is recorded.
pub(crate) fn deserialize_with_enum_warnings(
    value: TomlValue,
) -> Result<(TomlValue, ConfigToml, Vec<String>), toml::de::Error> {
    let mut sanitized = value;
    let warnings = remove_invalid_enum_values(&mut sanitized);
    let parsed = sanitized.clone().try_into::<ConfigToml>()?;
    Ok((sanitized, parsed, warnings))
}

fn remove_invalid_enum_values(value: &mut TomlValue) -> Vec<String> {
    let mut warnings = Vec::new();
    let path = Vec::new();
    scan_config_toml(value, &path, &mut warnings);
    warnings
}

fn scan_config_toml(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "approval_policy" => AskForApproval,
        "approvals_reviewer" => ApprovalsReviewer,
        "sandbox_mode" => SandboxMode,
        "forced_login_method" => ForcedLoginMethod,
        "cli_auth_credentials_store" => AuthCredentialsStoreMode,
        "mcp_oauth_credentials_store" => OAuthCredentialsStoreMode,
        "file_opener" => UriBasedFileOpener,
        "model_reasoning_effort" => ReasoningEffort,
        "plan_mode_reasoning_effort" => ReasoningEffort,
        "model_reasoning_summary" => ReasoningSummary,
        "model_verbosity" => Verbosity,
        "personality" => Personality,
        "service_tier" => ServiceTier,
        "experimental_thread_store" => ThreadStoreToml,
        "web_search" => WebSearchMode,
    );

    scan_shell_environment_policy(value, &child_path(path, SHELL_ENVIRONMENT_POLICY), warnings);
    scan_history(value, &child_path(path, "history"), warnings);
    scan_tui(value, &child_path(path, "tui"), warnings);
    scan_realtime(value, &child_path(path, "realtime"), warnings);
    scan_tools(value, &child_path(path, "tools"), warnings);
    scan_windows(value, &child_path(path, "windows"), warnings);
    scan_profiles(value, &child_path(path, "profiles"), warnings);
    scan_mcp_servers(value, &child_path(path, "mcp_servers"), warnings);
    scan_apps(value, &child_path(path, "apps"), warnings);
    scan_plugins(value, &child_path(path, "plugins"), warnings);
    scan_marketplaces(value, &child_path(path, "marketplaces"), warnings);
}

fn scan_shell_environment_policy(
    value: &mut TomlValue,
    path: &[String],
    warnings: &mut Vec<String>,
) {
    check_fields!(
        value,
        path,
        warnings,
        "inherit" => ShellEnvironmentPolicyInherit,
    );
}

fn scan_history(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "persistence" => HistoryPersistence,
    );
}

fn scan_tui(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "notifications" => Notifications,
        "notification_method" => NotificationMethod,
        "notification_condition" => NotificationCondition,
        "alternate_screen" => AltScreenMode,
        "session_picker_view" => SessionPickerViewMode,
    );
}

fn scan_realtime(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "version" => RealtimeWsVersion,
        "type" => RealtimeWsMode,
        "transport" => RealtimeTransport,
        "voice" => RealtimeVoice,
    );
}

fn scan_tools(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        &child_path(path, "web_search"),
        warnings,
        CONTEXT_SIZE => WebSearchContextSize,
    );
}

fn scan_windows(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "sandbox" => WindowsSandboxModeToml,
    );
}

fn scan_profiles(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    for profile_path in child_table_paths(value, path) {
        scan_profile(value, &profile_path, warnings);
    }
}

fn scan_profile(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    check_fields!(
        value,
        path,
        warnings,
        "service_tier" => ServiceTier,
        "approval_policy" => AskForApproval,
        "approvals_reviewer" => ApprovalsReviewer,
        "sandbox_mode" => SandboxMode,
        "model_reasoning_effort" => ReasoningEffort,
        "plan_mode_reasoning_effort" => ReasoningEffort,
        "model_reasoning_summary" => ReasoningSummary,
        "model_verbosity" => Verbosity,
        "personality" => Personality,
        "web_search" => WebSearchMode,
    );
    scan_tools(value, &child_path(path, "tools"), warnings);
    scan_windows(value, &child_path(path, "windows"), warnings);
    check_fields!(
        value,
        &child_path(path, "tui"),
        warnings,
        "session_picker_view" => SessionPickerViewMode,
    );
}

fn scan_mcp_servers(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    for server_path in child_table_paths(value, path) {
        scan_tool_approval_overrides(value, &server_path, warnings);
    }
}

fn scan_apps(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    for app_path in child_table_paths(value, path) {
        scan_tool_approval_overrides(value, &app_path, warnings);
    }
}

fn scan_plugins(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    for plugin_path in child_table_paths(value, path) {
        for server_path in child_table_paths(value, &child_path(&plugin_path, "mcp_servers")) {
            scan_tool_approval_overrides(value, &server_path, warnings);
        }
    }
}

fn scan_tool_approval_overrides(
    value: &mut TomlValue,
    path: &[String],
    warnings: &mut Vec<String>,
) {
    check_fields!(
        value,
        path,
        warnings,
        DEFAULT_TOOLS_APPROVAL_MODE => AppToolApproval,
    );
    for tool_path in child_table_paths(value, &child_path(path, "tools")) {
        check_fields!(
            value,
            &tool_path,
            warnings,
            APPROVAL_MODE => AppToolApproval,
        );
    }
}

fn scan_marketplaces(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>) {
    for marketplace_path in child_table_paths(value, path) {
        check_fields!(
            value,
            &marketplace_path,
            warnings,
            "source_type" => MarketplaceSourceType,
        );
    }
}

/// Checks one known enum leaf by parsing the raw TOML value as the real field type.
fn check_value<T>(value: &mut TomlValue, path: &[String], warnings: &mut Vec<String>)
where
    T: DeserializeOwned,
{
    let Some(raw_value) = value_at_path(value, path) else {
        return;
    };
    if raw_value.clone().try_into::<T>().is_ok() {
        return;
    }
    let Some(invalid_value) = remove_value_at_path(value, path) else {
        return;
    };
    warnings.push(format!(
        "Ignoring invalid config value at {}: {invalid_value}",
        display_path(path)
    ));
}

/// Returns table child paths in TOML map order without holding a borrow.
fn child_table_paths(value: &TomlValue, path: &[String]) -> Vec<Vec<String>> {
    let Some(table) = value_at_path(value, path).and_then(TomlValue::as_table) else {
        return Vec::new();
    };
    table
        .iter()
        .filter_map(|(key, value)| value.is_table().then(|| child_path(path, key)))
        .collect()
}

fn value_at_path<'a>(value: &'a TomlValue, path: &[String]) -> Option<&'a TomlValue> {
    let mut current = value;
    for key in path {
        current = current.as_table()?.get(key)?;
    }
    Some(current)
}

fn remove_value_at_path(value: &mut TomlValue, path: &[String]) -> Option<TomlValue> {
    let (last, parents) = path.split_last()?;
    let mut current = value;
    for key in parents {
        current = current.as_table_mut()?.get_mut(key)?;
    }
    current.as_table_mut()?.remove(last)
}

fn child_path(path: &[String], key: &str) -> Vec<String> {
    let mut child = path.to_vec();
    child.push(key.to_string());
    child
}

fn display_path(path: &[String]) -> String {
    path.iter()
        .map(|key| display_key(key))
        .collect::<Vec<_>>()
        .join(".")
}

fn display_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return key.to_string();
    }
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
