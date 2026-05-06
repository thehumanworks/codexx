use std::collections::HashMap;

use crate::config_toml::RealtimeTransport;
use crate::config_toml::RealtimeWsMode;
use crate::config_toml::RealtimeWsVersion;
use crate::config_toml::ThreadStoreToml;
use crate::config_toml::config_toml_fields;
use crate::profile_toml::config_profile_fields;
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
    let mut warnings = Vec::new();
    lenient_config.push_invalid_enum_warnings(&mut warnings, &mut WarningPath::default());
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

macro_rules! define_lenient_struct_from_config_fields {
    ($source_name:ident => $name:ident { $($entries:tt)* }) => {
        define_lenient_struct_from_config_fields! {
            @fields $name
            [$($entries)*]
            []
            $($entries)*
        }
    };
    (
        @fields $name:ident
        [$($entries:tt)*]
        [$($fields:tt)*]
    ) => {
        #[derive(Deserialize, Default)]
        struct $name {
            $($fields)*
        }

        impl $name {
            /// Adds warnings for this section and any nested sections.
            fn push_invalid_enum_warnings(
                &self,
                warnings: &mut Vec<InvalidEnumWarning>,
                path: &mut WarningPath,
            ) {
                let source = self;
                walk_lenient_config_fields!(source, warnings, path, $($entries)*);
            }
        }
    };
    (
        @fields $name:ident
        [$($entries:tt)*]
        [$($fields:tt)*]
        enum {
            $(#[$meta:meta])*
            pub $field:ident: Option<$ty:ty>,
        }
        $($rest:tt)*
    ) => {
        define_lenient_struct_from_config_fields! {
            @fields $name
            [$($entries)*]
            [
                $($fields)*
                $field: Option<Lenient<$ty>>,
            ]
            $($rest)*
        }
    };
    (
        @fields $name:ident
        [$($entries:tt)*]
        [$($fields:tt)*]
        section($lenient_ty:ident) {
            $(#[$meta:meta])*
            pub $field:ident: $ty:ty,
        }
        $($rest:tt)*
    ) => {
        define_lenient_struct_from_config_fields! {
            @fields $name
            [$($entries)*]
            [
                $($fields)*
                $field: Option<$lenient_ty>,
            ]
            $($rest)*
        }
    };
    (
        @fields $name:ident
        [$($entries:tt)*]
        [$($fields:tt)*]
        map($lenient_ty:ident) {
            $(#[$meta:meta])*
            pub $field:ident: HashMap<String, $ty:ty>,
        }
        $($rest:tt)*
    ) => {
        define_lenient_struct_from_config_fields! {
            @fields $name
            [$($entries)*]
            [
                $($fields)*
                #[serde(default)]
                $field: HashMap<String, $lenient_ty>,
            ]
            $($rest)*
        }
    };
    (
        @fields $name:ident
        [$($entries:tt)*]
        [$($fields:tt)*]
        direct {
            $(#[$meta:meta])*
            pub $field:ident: $ty:ty,
        }
        $($rest:tt)*
    ) => {
        define_lenient_struct_from_config_fields! {
            @fields $name
            [$($entries)*]
            [$($fields)*]
            $($rest)*
        }
    };
}

macro_rules! walk_lenient_config_fields {
    ($source:ident, $warnings:ident, $path:ident,) => {};
    (
        $source:ident,
        $warnings:ident,
        $path:ident,
        enum {
            $(#[$meta:meta])*
            pub $field:ident: Option<$ty:ty>,
        }
        $($rest:tt)*
    ) => {
        push_invalid_field($warnings, $path, stringify!($field), &$source.$field);
        walk_lenient_config_fields!($source, $warnings, $path, $($rest)*);
    };
    (
        $source:ident,
        $warnings:ident,
        $path:ident,
        section($lenient_ty:ident) {
            $(#[$meta:meta])*
            pub $field:ident: $ty:ty,
        }
        $($rest:tt)*
    ) => {
        if let Some(section) = &$source.$field {
            $path.push(stringify!($field));
            section.push_invalid_enum_warnings($warnings, $path);
            $path.pop();
        }
        walk_lenient_config_fields!($source, $warnings, $path, $($rest)*);
    };
    (
        $source:ident,
        $warnings:ident,
        $path:ident,
        map($lenient_ty:ident) {
            $(#[$meta:meta])*
            pub $field:ident: HashMap<String, $ty:ty>,
        }
        $($rest:tt)*
    ) => {
        let mut entries = $source.$field.iter().collect::<Vec<_>>();
        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        $path.push(stringify!($field));
        for (name, section) in entries {
            $path.push(name);
            section.push_invalid_enum_warnings($warnings, $path);
            $path.pop();
        }
        $path.pop();
        walk_lenient_config_fields!($source, $warnings, $path, $($rest)*);
    };
    (
        $source:ident,
        $warnings:ident,
        $path:ident,
        direct {
            $(#[$meta:meta])*
            pub $field:ident: $ty:ty,
        }
        $($rest:tt)*
    ) => {
        walk_lenient_config_fields!($source, $warnings, $path, $($rest)*);
    };
}

/// Defines the private lenient mirror from one TOML field list.
///
/// Each entry emits both the sparse struct field used for deserialization and
/// the warning traversal for that same TOML path.
macro_rules! define_lenient_config_shape {
    (
        $(
            $(#[$struct_meta:meta])*
            struct $name:ident {
                enums {
                    $(
                        $enum_key:literal => $enum_field:ident : $enum_ty:ty,
                    )*
                }
                sections {
                    $(
                        $(#[$section_attr:meta])*
                        $section_key:literal => $section_field:ident : $section_ty:ident,
                    )*
                }
                maps {
                    $(
                        $map_key:literal => $map_field:ident : $map_ty:ident,
                    )*
                }
            }
        )+
    ) => {
        $(
            $(#[$struct_meta])*
            #[derive(Deserialize, Default)]
            struct $name {
                $(
                    #[serde(rename = $enum_key)]
                    $enum_field: Option<Lenient<$enum_ty>>,
                )*
                $(
                    $(#[$section_attr])*
                    #[serde(rename = $section_key)]
                    $section_field: Option<$section_ty>,
                )*
                $(
                    #[serde(default, rename = $map_key)]
                    $map_field: HashMap<String, $map_ty>,
                )*
            }

            impl $name {
                /// Adds warnings for this section and any nested sections.
                fn push_invalid_enum_warnings(
                    &self,
                    warnings: &mut Vec<InvalidEnumWarning>,
                    path: &mut WarningPath,
                ) {
                    $(
                        push_invalid_field(warnings, path, $enum_key, &self.$enum_field);
                    )*
                    $(
                        if let Some(section) = &self.$section_field {
                            path.push($section_key);
                            section.push_invalid_enum_warnings(warnings, path);
                            path.pop();
                        }
                    )*
                    $(
                        let mut entries = self.$map_field.iter().collect::<Vec<_>>();
                        entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                        path.push($map_key);
                        for (name, section) in entries {
                            path.push(name);
                            section.push_invalid_enum_warnings(warnings, path);
                            path.pop();
                        }
                        path.pop();
                    )*
                }
            }
        )+
    };
}

config_toml_fields!(define_lenient_struct_from_config_fields);
config_profile_fields!(define_lenient_struct_from_config_fields);

define_lenient_config_shape! {
    /// Sparse mirror of `[history]` for the one enum-valued field inside it.
    struct LenientHistory {
        enums {
            "persistence" => persistence: HistoryPersistence,
        }
        sections {}
        maps {}
    }

    /// Sparse mirror of `[shell_environment_policy]` for enum handling.
    struct LenientShellEnvironmentPolicyToml {
        enums {
            "inherit" => inherit: ShellEnvironmentPolicyInherit,
        }
        sections {}
        maps {}
    }

    /// Sparse mirror of `[tools]` for enum-valued nested tool settings.
    struct LenientToolsToml {
        enums {}
        sections {
            #[serde(default, deserialize_with = "deserialize_lenient_web_search_tool")]
            "web_search" => web_search: LenientWebSearchToolConfig,
        }
        maps {}
    }

    /// Sparse mirror of `WebSearchToolConfig` for `context_size`.
    struct LenientWebSearchToolConfig {
        enums {
            "context_size" => context_size: WebSearchContextSize,
        }
        sections {}
        maps {}
    }

    struct LenientTui {
        enums {
            "notifications" => notifications: Notifications,
            "notification_method" => method: NotificationMethod,
            "notification_condition" => condition: NotificationCondition,
            "alternate_screen" => alternate_screen: AltScreenMode,
        }
        sections {}
        maps {}
    }

    struct LenientRealtimeToml {
        enums {
            "version" => version: RealtimeWsVersion,
            "type" => session_type: RealtimeWsMode,
            "transport" => transport: RealtimeTransport,
            "voice" => voice: RealtimeVoice,
        }
        sections {}
        maps {}
    }

    struct LenientWindowsToml {
        enums {
            "sandbox" => sandbox: WindowsSandboxModeToml,
        }
        sections {}
        maps {}
    }
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

/// Result of checking one enum-valued field.
///
/// The typed value is intentionally discarded after parsing: the strict
/// `ConfigToml` deserialization below is the only place that should produce
/// runtime config values.
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

/// Captures everything needed to remove an invalid enum leaf and report it
/// through the startup warning path.
struct InvalidEnumWarning {
    segments: Vec<String>,
    invalid_value: TomlValue,
}

/// Mutable TOML path shared by generated lenient-config walkers.
#[derive(Default)]
struct WarningPath {
    segments: Vec<String>,
}

impl WarningPath {
    /// Descends into a child table or map entry.
    fn push(&mut self, segment: impl AsRef<str>) {
        self.segments.push(segment.as_ref().to_string());
    }

    /// Returns to the parent section after a nested walk.
    fn pop(&mut self) {
        self.segments.pop();
    }

    /// Builds the full warning path for one invalid leaf.
    fn with_leaf(&self, segment: &str) -> Vec<String> {
        let mut segments = self.segments.clone();
        segments.push(segment.to_string());
        segments
    }
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
fn push_invalid_field<T>(
    warnings: &mut Vec<InvalidEnumWarning>,
    path: &WarningPath,
    segment: &str,
    value: &Option<Lenient<T>>,
) {
    let Some(invalid_value) = value.as_ref().and_then(Lenient::invalid_value) else {
        return;
    };
    warnings.push(InvalidEnumWarning {
        segments: path.with_leaf(segment),
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
