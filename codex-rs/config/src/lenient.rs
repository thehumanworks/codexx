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

use self::PathSegment::AnyTable;
use self::PathSegment::Key;

type FieldParser = fn(&TomlValue) -> bool;
type EnumFieldSpec = (&'static [PathSegment], FieldParser);

/// One segment in a known enum-valued config path.
enum PathSegment {
    /// Match this literal table key.
    Key(&'static str),
    /// Expand across every table child at this point, such as `profiles.*`.
    AnyTable,
}

static ENUM_FIELD_SPECS: &[EnumFieldSpec] = &[
    (&[Key("approval_policy")], parses_as::<AskForApproval>),
    (&[Key("approvals_reviewer")], parses_as::<ApprovalsReviewer>),
    (&[Key("sandbox_mode")], parses_as::<SandboxMode>),
    (
        &[Key("forced_login_method")],
        parses_as::<ForcedLoginMethod>,
    ),
    (
        &[Key("cli_auth_credentials_store")],
        parses_as::<AuthCredentialsStoreMode>,
    ),
    (
        &[Key("mcp_oauth_credentials_store")],
        parses_as::<OAuthCredentialsStoreMode>,
    ),
    (&[Key("file_opener")], parses_as::<UriBasedFileOpener>),
    (
        &[Key("model_reasoning_effort")],
        parses_as::<ReasoningEffort>,
    ),
    (
        &[Key("plan_mode_reasoning_effort")],
        parses_as::<ReasoningEffort>,
    ),
    (
        &[Key("model_reasoning_summary")],
        parses_as::<ReasoningSummary>,
    ),
    (&[Key("model_verbosity")], parses_as::<Verbosity>),
    (&[Key("personality")], parses_as::<Personality>),
    (&[Key("service_tier")], parses_as::<ServiceTier>),
    (
        &[Key("experimental_thread_store")],
        parses_as::<ThreadStoreToml>,
    ),
    (&[Key("web_search")], parses_as::<WebSearchMode>),
    (
        &[Key("shell_environment_policy"), Key("inherit")],
        parses_as::<ShellEnvironmentPolicyInherit>,
    ),
    (
        &[Key("history"), Key("persistence")],
        parses_as::<HistoryPersistence>,
    ),
    (
        &[Key("tui"), Key("notifications")],
        parses_as::<Notifications>,
    ),
    (
        &[Key("tui"), Key("notification_method")],
        parses_as::<NotificationMethod>,
    ),
    (
        &[Key("tui"), Key("notification_condition")],
        parses_as::<NotificationCondition>,
    ),
    (
        &[Key("tui"), Key("alternate_screen")],
        parses_as::<AltScreenMode>,
    ),
    (
        &[Key("tui"), Key("session_picker_view")],
        parses_as::<SessionPickerViewMode>,
    ),
    (
        &[Key("realtime"), Key("version")],
        parses_as::<RealtimeWsVersion>,
    ),
    (&[Key("realtime"), Key("type")], parses_as::<RealtimeWsMode>),
    (
        &[Key("realtime"), Key("transport")],
        parses_as::<RealtimeTransport>,
    ),
    (&[Key("realtime"), Key("voice")], parses_as::<RealtimeVoice>),
    (
        &[Key("tools"), Key("web_search"), Key("context_size")],
        parses_as::<WebSearchContextSize>,
    ),
    (
        &[Key("windows"), Key("sandbox")],
        parses_as::<WindowsSandboxModeToml>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("service_tier")],
        parses_as::<ServiceTier>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("approval_policy")],
        parses_as::<AskForApproval>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("approvals_reviewer")],
        parses_as::<ApprovalsReviewer>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("sandbox_mode")],
        parses_as::<SandboxMode>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("model_reasoning_effort")],
        parses_as::<ReasoningEffort>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("plan_mode_reasoning_effort")],
        parses_as::<ReasoningEffort>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("model_reasoning_summary")],
        parses_as::<ReasoningSummary>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("model_verbosity")],
        parses_as::<Verbosity>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("personality")],
        parses_as::<Personality>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("web_search")],
        parses_as::<WebSearchMode>,
    ),
    (
        &[
            Key("profiles"),
            AnyTable,
            Key("tools"),
            Key("web_search"),
            Key("context_size"),
        ],
        parses_as::<WebSearchContextSize>,
    ),
    (
        &[Key("profiles"), AnyTable, Key("windows"), Key("sandbox")],
        parses_as::<WindowsSandboxModeToml>,
    ),
    (
        &[
            Key("profiles"),
            AnyTable,
            Key("tui"),
            Key("session_picker_view"),
        ],
        parses_as::<SessionPickerViewMode>,
    ),
    (
        &[
            Key("mcp_servers"),
            AnyTable,
            Key("default_tools_approval_mode"),
        ],
        parses_as::<AppToolApproval>,
    ),
    (
        &[
            Key("mcp_servers"),
            AnyTable,
            Key("tools"),
            AnyTable,
            Key("approval_mode"),
        ],
        parses_as::<AppToolApproval>,
    ),
    (
        &[Key("apps"), AnyTable, Key("default_tools_approval_mode")],
        parses_as::<AppToolApproval>,
    ),
    (
        &[
            Key("apps"),
            AnyTable,
            Key("tools"),
            AnyTable,
            Key("approval_mode"),
        ],
        parses_as::<AppToolApproval>,
    ),
    (
        &[
            Key("plugins"),
            AnyTable,
            Key("mcp_servers"),
            AnyTable,
            Key("default_tools_approval_mode"),
        ],
        parses_as::<AppToolApproval>,
    ),
    (
        &[
            Key("plugins"),
            AnyTable,
            Key("mcp_servers"),
            AnyTable,
            Key("tools"),
            AnyTable,
            Key("approval_mode"),
        ],
        parses_as::<AppToolApproval>,
    ),
    (
        &[Key("marketplaces"), AnyTable, Key("source_type")],
        parses_as::<MarketplaceSourceType>,
    ),
];

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
    let mut warnings = Vec::new();
    for (spec_path, parser) in ENUM_FIELD_SPECS {
        for path in matching_paths(&sanitized, spec_path) {
            remove_if_invalid(&mut sanitized, &path, *parser, &mut warnings);
        }
    }
    let parsed = sanitized.clone().try_into::<ConfigToml>()?;
    Ok((sanitized, parsed, warnings))
}

fn parses_as<T>(value: &TomlValue) -> bool
where
    T: DeserializeOwned,
{
    value.clone().try_into::<T>().is_ok()
}

/// Removes one known enum leaf when its raw TOML value does not parse as its real type.
fn remove_if_invalid(
    value: &mut TomlValue,
    path: &[String],
    parser: FieldParser,
    warnings: &mut Vec<String>,
) {
    let Some(raw_value) = value_at_path(value, path) else {
        return;
    };
    if parser(raw_value) {
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

/// Expands a spec path into concrete TOML paths without holding borrows across mutation.
fn matching_paths(value: &TomlValue, spec_path: &[PathSegment]) -> Vec<Vec<String>> {
    let mut paths = vec![Vec::new()];
    for segment in spec_path {
        match segment {
            Key(key) => {
                for path in &mut paths {
                    path.push((*key).to_string());
                }
            }
            AnyTable => {
                let mut expanded = Vec::new();
                for path in paths {
                    let Some(table) = value_at_path(value, &path).and_then(TomlValue::as_table)
                    else {
                        continue;
                    };
                    expanded.extend(table.iter().filter_map(|(key, value)| {
                        if !value.is_table() {
                            return None;
                        }
                        let mut child = path.clone();
                        child.push(key.clone());
                        Some(child)
                    }));
                }
                paths = expanded;
            }
        }
    }
    paths
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
