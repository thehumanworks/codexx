use crate::types::Notifications;
use codex_protocol::config_types::WebSearchContextSize;
use serde::de::DeserializeOwned;
use serde_path_to_error::Path as SerdePath;
use serde_path_to_error::Segment as SerdeSegment;
use toml::Value as TomlValue;

const WEB_SEARCH_TOOL_CONFIG_INPUT_ERROR: &str =
    "data did not match any variant of untagged enum WebSearchToolConfigInput";
const NOTIFICATIONS_ERROR: &str = "data did not match any variant of untagged enum Notifications";
const CONTEXT_SIZE_KEY: &str = "context_size";
const NOTIFICATIONS_KEY: &str = "notifications";

/// Deserializes TOML while turning invalid enum-valued settings into warnings.
///
/// Serde reports enum failures as normal deserialization errors, which would
/// otherwise reject the whole assembled config. Config files are user-editable,
/// so this helper keeps the strongly typed destination type and handles enum
/// leniency at the load boundary: deserialize, find the first invalid enum
/// value, remove only that TOML key, record a startup warning, and retry.
pub(crate) fn deserialize_with_enum_warnings<T>(
    mut value: TomlValue,
) -> Result<(TomlValue, T, Vec<String>), toml::de::Error>
where
    T: DeserializeOwned,
{
    let mut warnings = Vec::new();

    loop {
        // serde_path_to_error gives us the config path that failed, so we can
        // remove the offending TOML value without making invalid enum state
        // representable inside ConfigToml or its nested structs.
        match serde_path_to_error::deserialize(value.clone()) {
            Ok(parsed) => return Ok((value, parsed, warnings)),
            Err(err) => {
                let Some(path) = path_segments(err.path()) else {
                    return Err(err.into_inner());
                };
                let toml_error = err.into_inner();

                let removed = if is_unknown_variant_error(&toml_error) {
                    remove_unknown_variant_value::<T>(
                        &mut value,
                        &path,
                        unknown_variant(&toml_error),
                        toml_error.message(),
                    )
                } else if is_web_search_tool_config_input_error(&toml_error) {
                    remove_invalid_web_search_context_size(&mut value, &path)
                } else if is_notifications_error(&toml_error) {
                    remove_invalid_notifications_value(&mut value, &path)
                } else {
                    None
                };

                let Some((removed_path, invalid_value)) = removed else {
                    return Err(toml_error);
                };
                warnings.push(format!(
                    "Ignoring invalid config value at {}: {invalid_value}",
                    display_path(&removed_path)
                ));
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PathSegment {
    Key(String),
    Index(usize),
}

/// Converts Serde's structured path into the TOML traversal path used below.
///
/// Root-level or unknown paths are not safe to remove because they do not name
/// a specific user-authored key.
fn path_segments(path: &SerdePath) -> Option<Vec<PathSegment>> {
    let mut segments = Vec::new();
    for segment in path {
        match segment {
            SerdeSegment::Map { key } | SerdeSegment::Enum { variant: key } => {
                segments.push(PathSegment::Key(key.clone()));
            }
            SerdeSegment::Seq { index } => segments.push(PathSegment::Index(*index)),
            SerdeSegment::Unknown => return None,
        }
    }
    (!segments.is_empty()).then_some(segments)
}

/// Detects Serde's enum-variant failures without swallowing unrelated errors.
///
/// toml::de::Error does not expose a structured enum-kind discriminator, so the
/// loader keeps the match deliberately narrow and lets all other parse and type
/// errors flow through the existing config-error path.
fn is_unknown_variant_error(err: &toml::de::Error) -> bool {
    err.message().contains("unknown variant")
}

/// Extracts the rejected enum variant from Serde's diagnostic string.
///
/// The TOML error type does not expose this value directly, but Serde formats
/// enum errors consistently as `unknown variant \`value\``. The value lets us
/// recover a leaf key when Serde reports the containing table for flattened
/// config structs.
fn unknown_variant(err: &toml::de::Error) -> Option<&str> {
    let message = err.message();
    let variant = message.strip_prefix("unknown variant `")?;
    let (variant, _) = variant.split_once('`')?;
    Some(variant)
}

/// Detects the custom `tools.web_search` deserializer's collapsed error.
fn is_web_search_tool_config_input_error(err: &toml::de::Error) -> bool {
    err.message() == WEB_SEARCH_TOOL_CONFIG_INPUT_ERROR
}

/// Detects the untagged TUI notification setting error.
fn is_notifications_error(err: &toml::de::Error) -> bool {
    err.message() == NOTIFICATIONS_ERROR
}

/// Removes an ordinary enum leaf, but only when the TOML value matches the
/// rejected variant Serde reported.
fn remove_unknown_variant_value<T>(
    value: &mut TomlValue,
    path: &[PathSegment],
    unknown_variant: Option<&str>,
    original_message: &str,
) -> Option<(Vec<PathSegment>, TomlValue)>
where
    T: DeserializeOwned,
{
    let variant = unknown_variant?;
    if value_at_segments(value, path).and_then(TomlValue::as_str) == Some(variant) {
        return remove_value_at_segments(value, path).map(|removed| (path.to_vec(), removed));
    }
    remove_direct_string_child_that_unblocks::<T>(value, path, variant, original_message)
}

/// Removes `tools.web_search.context_size` after the untagged table/boolean
/// deserializer hides the inner enum error.
///
/// The explicit path keeps this from swallowing unrelated `tools.web_search`
/// shape errors such as a bad `allowed_domains` value.
fn remove_invalid_web_search_context_size(
    value: &mut TomlValue,
    web_search_path: &[PathSegment],
) -> Option<(Vec<PathSegment>, TomlValue)> {
    remove_invalid_leaf_value::<WebSearchContextSize>(value, web_search_path, CONTEXT_SIZE_KEY)
}

/// Removes `tui.notifications` when its untagged bool/list parser fails.
///
/// This is separate from ordinary enum handling because Serde's untagged error
/// does not include the rejected value text. The candidate path is constrained
/// to the `notifications` leaf so a flattened-parent error cannot delete the
/// entire `[tui]` table.
fn remove_invalid_notifications_value(
    value: &mut TomlValue,
    path: &[PathSegment],
) -> Option<(Vec<PathSegment>, TomlValue)> {
    remove_invalid_leaf_value::<Notifications>(value, path, NOTIFICATIONS_KEY)
}

/// Reads a TOML value by structured path without flattening map keys into text.
fn value_at_segments<'a>(value: &'a TomlValue, segments: &[PathSegment]) -> Option<&'a TomlValue> {
    let mut current = value;
    for segment in segments {
        match segment {
            PathSegment::Key(key) => current = current.as_table()?.get(key)?,
            PathSegment::Index(index) => current = current.as_array()?.get(*index)?,
        }
    }
    Some(current)
}

/// Removes a flattened enum child only after proving it unblocks parsing.
///
/// For `#[serde(flatten)]`, serde_path_to_error can report the containing
/// table instead of the enum leaf. Recursing through the whole subtree can
/// remove the wrong setting, so this fallback only tries direct string children
/// with the rejected value and accepts exactly one child that moves
/// deserialization past the original error.
fn remove_direct_string_child_that_unblocks<T>(
    value: &mut TomlValue,
    path: &[PathSegment],
    variant: &str,
    original_message: &str,
) -> Option<(Vec<PathSegment>, TomlValue)>
where
    T: DeserializeOwned,
{
    let mut unblocking_key = None;
    for end in (0..=path.len()).rev() {
        let parent_path = &path[..end];
        let Some(table) = value_at_segments(value, parent_path).and_then(TomlValue::as_table)
        else {
            continue;
        };
        let keys = table
            .iter()
            .filter_map(|(key, child)| (child.as_str() == Some(variant)).then_some(key.clone()))
            .collect::<Vec<_>>();

        for key in keys {
            let mut child_path = parent_path.to_vec();
            child_path.push(PathSegment::Key(key.clone()));

            let mut candidate_value = value.clone();
            remove_value_at_segments(&mut candidate_value, &child_path)?;
            let parsed: Result<T, serde_path_to_error::Error<toml::de::Error>> =
                serde_path_to_error::deserialize(candidate_value);
            let removes_original_error = match parsed {
                Ok(_) => true,
                Err(err) => {
                    path_segments(err.path()).as_deref() != Some(path)
                        || err.inner().message() != original_message
                }
            };
            if !removes_original_error {
                continue;
            }
            if unblocking_key
                .replace((parent_path.to_vec(), key))
                .is_some()
            {
                return None;
            }
        }
    }

    let (mut child_path, key) = unblocking_key?;
    child_path.push(PathSegment::Key(key));
    remove_value_at_segments(value, &child_path).map(|removed| (child_path, removed))
}

/// Removes a named leaf after proving that leaf still fails typed parsing.
///
/// Untagged enum errors can point at a containing table, or at an internal
/// deserializer segment that is not a real TOML key. Walking ancestors lets us
/// recover `path.leaf` without ever searching unrelated descendants.
fn remove_invalid_leaf_value<T>(
    value: &mut TomlValue,
    path: &[PathSegment],
    leaf: &str,
) -> Option<(Vec<PathSegment>, TomlValue)>
where
    T: DeserializeOwned,
{
    for end in (0..=path.len()).rev() {
        let mut candidate = path[..end].to_vec();
        if !candidate
            .last()
            .is_some_and(|segment| matches!(segment, PathSegment::Key(key) if key == leaf))
        {
            candidate.push(PathSegment::Key(leaf.to_string()));
        }

        let parsed: Result<T, toml::de::Error> = {
            let Some(raw_value) = value_at_segments(value, &candidate) else {
                continue;
            };
            raw_value.clone().try_into()
        };
        if parsed.is_ok() {
            return None;
        }
        return remove_value_at_segments(value, &candidate).map(|removed| (candidate, removed));
    }

    None
}

/// Deletes the offending TOML value by structured path.
fn remove_value_at_segments(value: &mut TomlValue, segments: &[PathSegment]) -> Option<TomlValue> {
    let (last, parents) = segments.split_last()?;
    let mut current = value;
    for segment in parents {
        match segment {
            PathSegment::Key(key) => current = current.as_table_mut()?.get_mut(key)?,
            PathSegment::Index(index) => current = current.as_array_mut()?.get_mut(*index)?,
        }
    }
    match last {
        PathSegment::Key(key) => current.as_table_mut()?.remove(key),
        PathSegment::Index(index) => {
            let array = current.as_array_mut()?;
            if *index >= array.len() {
                return None;
            }
            Some(array.remove(*index))
        }
    }
}

/// Formats paths for warnings, quoting literal table keys that contain dots.
fn display_path(segments: &[PathSegment]) -> String {
    let mut path = String::new();
    for segment in segments {
        match segment {
            PathSegment::Key(key) => {
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(&display_key(key));
            }
            PathSegment::Index(index) => path.push_str(&format!("[{index}]")),
        }
    }
    path
}

/// Formats one TOML key segment for warning paths.
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
