use std::collections::HashMap;

use crate::config_toml::RealtimeTransport;
use crate::config_toml::RealtimeWsMode;
use crate::config_toml::RealtimeWsVersion;
use crate::config_toml::ThreadStoreToml;
use crate::types::AltScreenMode;
use crate::types::ApprovalsReviewer;
use crate::types::AuthCredentialsStoreMode;
use crate::types::HistoryPersistence;
use crate::types::NotificationCondition;
use crate::types::NotificationMethod;
use crate::types::Notifications;
use crate::types::OAuthCredentialsStoreMode;
use crate::types::Personality;
use crate::types::ServiceTier;
use crate::types::UriBasedFileOpener;
use crate::types::WebSearchMode;
use crate::types::WindowsSandboxModeToml;
use codex_protocol::config_types::ForcedLoginMethod;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use codex_protocol::config_types::Verbosity;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::RealtimeVoice;
use serde::Deserialize;
use serde::Deserializer;
use toml::Value as TomlValue;

/// Deserializes TOML through a private lenient config shape, then returns a
/// sanitized strict config.
///
/// The important boundary is that invalid enum values are representable only in
/// the private `LenientConfigToml` mirror below. The public `ConfigToml` and all
/// runtime consumers still receive ordinary typed enum fields. When the
/// intermediate shape finds an invalid enum, this removes the raw TOML value,
/// records the same startup warning the retry-loop approach produced, and then
/// deserializes the sanitized TOML into the requested strict type.
pub(crate) fn deserialize_with_enum_warnings<T>(
    mut value: TomlValue,
) -> Result<(TomlValue, T, Vec<String>), toml::de::Error>
where
    T: serde::de::DeserializeOwned,
{
    let lenient_config: LenientConfigToml = value.clone().try_into()?;
    let warnings = lenient_config.invalid_enum_warnings();
    for warning in &warnings {
        remove_value_at_segments(&mut value, &warning.segments);
    }
    let parsed: T = value.clone().try_into()?;
    let messages = warnings
        .into_iter()
        .map(InvalidEnumWarning::message)
        .collect();
    Ok((value, parsed, messages))
}

/// Result of checking one enum-valued field.
///
/// The typed value is intentionally discarded: the strict `ConfigToml`
/// deserialization below is the only place that should produce runtime config
/// values. This wrapper only records the raw TOML value when the enum parse
/// fails so the loader can warn and remove that leaf.
#[derive(Debug, Clone, PartialEq)]
enum Lenient<T> {
    Valid(T),
    Invalid(TomlValue),
}

impl<T> Lenient<T> {
    /// Returns the original TOML value only when this enum field failed to
    /// parse; callers use this as the single warning signal.
    fn invalid_value(&self) -> Option<&TomlValue> {
        match self {
            Self::Valid(_) => None,
            Self::Invalid(value) => Some(value),
        }
    }
}

impl<'de, T> Deserialize<'de> for Lenient<T>
where
    T: serde::de::DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = TomlValue::deserialize(deserializer)?;
        let parsed: Result<T, toml::de::Error> = value.clone().try_into();
        Ok(match parsed {
            Ok(parsed) => Self::Valid(parsed),
            Err(_) => Self::Invalid(value),
        })
    }
}

/// Adds root-level invalid enum warnings without repeating the path and field
/// name at each callsite.
macro_rules! push_invalid_root_fields {
    ($warnings:expr, $source:expr, $($field:ident),+ $(,)?) => {
        $(
            push_invalid_field(
                &mut $warnings,
                &[stringify!($field)],
                &$source.$field,
            );
        )+
    };
}

/// Adds profile-scoped invalid enum warnings while preserving the profile name
/// as its own TOML path segment. Keeping it segmented matters because profile
/// names can contain dots.
macro_rules! push_invalid_profile_fields {
    ($warnings:expr, $source:expr, $profile:expr, $($field:ident),+ $(,)?) => {
        $(
            push_invalid_field(
                $warnings,
                &["profiles", $profile, stringify!($field)],
                &$source.$field,
            );
        )+
    };
}

/// Sparse mirror of `ConfigToml` containing only enum-valued config fields.
///
/// This deliberately ignores non-enum fields. The strict `ConfigToml`
/// deserialization after sanitization remains responsible for validating the
/// full config shape, unknown keys, and non-enum type errors.
#[derive(Deserialize, Default)]
struct LenientConfigToml {
    approval_policy: Option<Lenient<AskForApproval>>,
    approvals_reviewer: Option<Lenient<ApprovalsReviewer>>,
    sandbox_mode: Option<Lenient<SandboxMode>>,
    forced_login_method: Option<Lenient<ForcedLoginMethod>>,
    cli_auth_credentials_store: Option<Lenient<AuthCredentialsStoreMode>>,
    mcp_oauth_credentials_store: Option<Lenient<OAuthCredentialsStoreMode>>,
    file_opener: Option<Lenient<UriBasedFileOpener>>,
    model_reasoning_effort: Option<Lenient<ReasoningEffort>>,
    plan_mode_reasoning_effort: Option<Lenient<ReasoningEffort>>,
    model_reasoning_summary: Option<Lenient<ReasoningSummary>>,
    model_verbosity: Option<Lenient<Verbosity>>,
    personality: Option<Lenient<Personality>>,
    service_tier: Option<Lenient<ServiceTier>>,
    experimental_thread_store: Option<Lenient<ThreadStoreToml>>,
    web_search: Option<Lenient<WebSearchMode>>,
    history: Option<LenientHistory>,
    shell_environment_policy: Option<LenientShellEnvironmentPolicyToml>,
    tools: Option<LenientToolsToml>,
    tui: Option<LenientTui>,
    realtime: Option<LenientRealtimeToml>,
    windows: Option<LenientWindowsToml>,

    #[serde(default)]
    profiles: HashMap<String, LenientConfigProfile>,
}

impl LenientConfigToml {
    fn invalid_enum_warnings(&self) -> Vec<InvalidEnumWarning> {
        let mut warnings = Vec::new();
        push_invalid_root_fields!(
            warnings,
            self,
            approval_policy,
            approvals_reviewer,
            sandbox_mode,
            forced_login_method,
            cli_auth_credentials_store,
            mcp_oauth_credentials_store,
            file_opener,
            model_reasoning_effort,
            plan_mode_reasoning_effort,
            model_reasoning_summary,
            model_verbosity,
            personality,
            service_tier,
            experimental_thread_store,
            web_search
        );

        if let Some(history) = &self.history {
            push_invalid_field(
                &mut warnings,
                &["history", "persistence"],
                &history.persistence,
            );
        }
        if let Some(shell_environment_policy) = &self.shell_environment_policy {
            push_invalid_field(
                &mut warnings,
                &["shell_environment_policy", "inherit"],
                &shell_environment_policy.inherit,
            );
        }
        if let Some(tools) = &self.tools
            && let Some(web_search) = &tools.web_search
        {
            push_invalid_field(
                &mut warnings,
                &["tools", "web_search", "context_size"],
                &web_search.context_size,
            );
        }
        if let Some(tui) = &self.tui {
            push_invalid_field(&mut warnings, &["tui", "notifications"], &tui.notifications);
            push_invalid_field(&mut warnings, &["tui", "notification_method"], &tui.method);
            push_invalid_field(
                &mut warnings,
                &["tui", "notification_condition"],
                &tui.condition,
            );
            push_invalid_field(
                &mut warnings,
                &["tui", "alternate_screen"],
                &tui.alternate_screen,
            );
        }
        if let Some(realtime) = &self.realtime {
            push_invalid_field(&mut warnings, &["realtime", "version"], &realtime.version);
            push_invalid_field(&mut warnings, &["realtime", "type"], &realtime.session_type);
            push_invalid_field(
                &mut warnings,
                &["realtime", "transport"],
                &realtime.transport,
            );
            push_invalid_field(&mut warnings, &["realtime", "voice"], &realtime.voice);
        }
        if let Some(windows) = &self.windows {
            push_invalid_field(&mut warnings, &["windows", "sandbox"], &windows.sandbox);
        }
        let mut profiles = self.profiles.iter().collect::<Vec<_>>();
        profiles.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (name, profile) in profiles {
            profile.push_invalid_enum_warnings(&mut warnings, name);
        }
        warnings
    }
}

#[derive(Deserialize, Default)]
struct LenientConfigProfile {
    service_tier: Option<Lenient<ServiceTier>>,
    approval_policy: Option<Lenient<AskForApproval>>,
    approvals_reviewer: Option<Lenient<ApprovalsReviewer>>,
    sandbox_mode: Option<Lenient<SandboxMode>>,
    model_reasoning_effort: Option<Lenient<ReasoningEffort>>,
    plan_mode_reasoning_effort: Option<Lenient<ReasoningEffort>>,
    model_reasoning_summary: Option<Lenient<ReasoningSummary>>,
    model_verbosity: Option<Lenient<Verbosity>>,
    personality: Option<Lenient<Personality>>,
    tools: Option<LenientToolsToml>,
    web_search: Option<Lenient<WebSearchMode>>,
    windows: Option<LenientWindowsToml>,
}

impl LenientConfigProfile {
    fn push_invalid_enum_warnings(&self, warnings: &mut Vec<InvalidEnumWarning>, profile: &str) {
        push_invalid_profile_fields!(
            warnings,
            self,
            profile,
            service_tier,
            approval_policy,
            approvals_reviewer,
            sandbox_mode,
            model_reasoning_effort,
            plan_mode_reasoning_effort,
            model_reasoning_summary,
            model_verbosity,
            personality,
            web_search
        );
        if let Some(tools) = &self.tools
            && let Some(web_search) = &tools.web_search
        {
            push_invalid_field(
                warnings,
                &["profiles", profile, "tools", "web_search", "context_size"],
                &web_search.context_size,
            );
        }
        if let Some(windows) = &self.windows {
            push_invalid_field(
                warnings,
                &["profiles", profile, "windows", "sandbox"],
                &windows.sandbox,
            );
        }
    }
}

/// Sparse mirror of `[history]` for the one enum-valued field inside it.
#[derive(Deserialize, Default)]
struct LenientHistory {
    persistence: Option<Lenient<HistoryPersistence>>,
}

/// Sparse mirror of `[shell_environment_policy]` for enum handling.
#[derive(Deserialize, Default)]
struct LenientShellEnvironmentPolicyToml {
    inherit: Option<Lenient<ShellEnvironmentPolicyInherit>>,
}

/// Sparse mirror of `[tools]` for enum-valued nested tool settings.
#[derive(Deserialize, Default)]
struct LenientToolsToml {
    #[serde(default, deserialize_with = "deserialize_lenient_web_search_tool")]
    web_search: Option<LenientWebSearchToolConfig>,
}

/// Sparse mirror of `WebSearchToolConfig` for `context_size`.
#[derive(Deserialize)]
struct LenientWebSearchToolConfig {
    context_size: Option<Lenient<WebSearchContextSize>>,
}

/// Reads `tools.web_search` only when it is a table.
///
/// The real config also accepts a boolean for this field. A non-table,
/// non-boolean value remains a strict deserialization error later; this helper
/// exists only to peek into the table form and warn on its enum leaf.
fn deserialize_lenient_web_search_tool<'de, D>(
    deserializer: D,
) -> Result<Option<LenientWebSearchToolConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<TomlValue>::deserialize(deserializer)?;
    match value {
        Some(TomlValue::Table(table)) => {
            table.try_into().map(Some).map_err(serde::de::Error::custom)
        }
        Some(TomlValue::Boolean(_)) | None => Ok(None),
        Some(_) => Ok(None),
    }
}

#[derive(Deserialize, Default)]
struct LenientTui {
    #[serde(rename = "notifications")]
    notifications: Option<Lenient<Notifications>>,
    #[serde(rename = "notification_method")]
    method: Option<Lenient<NotificationMethod>>,
    #[serde(rename = "notification_condition")]
    condition: Option<Lenient<NotificationCondition>>,
    alternate_screen: Option<Lenient<AltScreenMode>>,
}

#[derive(Deserialize, Default)]
struct LenientRealtimeToml {
    version: Option<Lenient<RealtimeWsVersion>>,
    #[serde(rename = "type")]
    session_type: Option<Lenient<RealtimeWsMode>>,
    transport: Option<Lenient<RealtimeTransport>>,
    voice: Option<Lenient<RealtimeVoice>>,
}

#[derive(Deserialize, Default)]
struct LenientWindowsToml {
    sandbox: Option<Lenient<WindowsSandboxModeToml>>,
}

/// Captures everything needed to remove an invalid enum leaf and report it
/// through the startup warning path.
struct InvalidEnumWarning {
    segments: Vec<String>,
    invalid_value: TomlValue,
}

impl InvalidEnumWarning {
    /// Formats the exact warning string consumed by callers today.
    fn message(self) -> String {
        let Self {
            segments,
            invalid_value,
        } = self;
        let path = segments.join(".");
        format!("Ignoring invalid config value at {path}: {invalid_value}")
    }
}

/// Adds one warning if a lenient field contains an invalid raw TOML value.
fn push_invalid_field<T, S>(
    warnings: &mut Vec<InvalidEnumWarning>,
    segments: &[S],
    value: &Option<Lenient<T>>,
) where
    S: AsRef<str>,
{
    let Some(invalid_value) = value.as_ref().and_then(Lenient::invalid_value) else {
        return;
    };
    warnings.push(InvalidEnumWarning {
        segments: segments
            .iter()
            .map(|segment| segment.as_ref().to_string())
            .collect(),
        invalid_value: invalid_value.clone(),
    });
}

/// Deletes the offending TOML leaf before strict deserialization.
///
/// The input is already split into table segments, which avoids ambiguity for
/// keys like profile names or project paths that may contain literal dots.
fn remove_value_at_segments(value: &mut TomlValue, segments: &[String]) -> Option<TomlValue> {
    let (last, parents) = segments.split_last()?;
    let mut current = value;
    for segment in parents {
        current = current.as_table_mut()?.get_mut(segment)?;
    }
    current.as_table_mut()?.remove(last)
}
