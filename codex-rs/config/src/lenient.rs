use serde::de::DeserializeOwned;
use toml::Value as TomlValue;

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
        // representable inside ConfigToml or its nested structs. For flattened
        // nested structs Serde can report the containing table; the removal
        // helper uses the invalid enum variant text to narrow that to the leaf.
        match serde_path_to_error::deserialize(value.clone()) {
            Ok(parsed) => return Ok((value, parsed, warnings)),
            Err(err) => {
                let path = err.path().to_string();
                let toml_error = err.into_inner();
                if !is_unknown_variant_error(&toml_error) {
                    return Err(toml_error);
                }

                let unknown_variant = unknown_variant(&toml_error);
                let Some((removed_path, invalid_value)) =
                    remove_value_at_path(&mut value, &path, unknown_variant)
                else {
                    return Err(toml_error);
                };
                warnings.push(format!(
                    "Ignoring invalid config value at {removed_path}: {invalid_value}"
                ));
            }
        }
    }
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

/// Removes the failed TOML key identified by serde_path_to_error.
///
/// Config enum fields live in tables rather than arrays, so the path traversal
/// intentionally follows table keys only. If the path cannot be removed, the
/// caller falls back to returning the original deserialization error.
fn remove_value_at_path(
    value: &mut TomlValue,
    path: &str,
    unknown_variant: Option<&str>,
) -> Option<(String, TomlValue)> {
    let mut parts = path.split('.').peekable();
    let mut current = value;

    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let table = current.as_table_mut()?;
            if let Some(variant) = unknown_variant
                && let Some(candidate) = table.get_mut(part)
                && candidate.as_str() != Some(variant)
                && let Some(removed) = remove_matching_string_value(candidate, path, variant)
            {
                return Some(removed);
            }
            return table
                .remove(part)
                .map(|removed| (path.to_string(), removed));
        }
        current = current.as_table_mut()?.get_mut(part)?;
    }

    None
}

/// Removes the first nested string value that matches the rejected enum text.
///
/// This is only a fallback for flattened nested structs where Serde reports the
/// parent table path. Exact-path removals still win whenever Serde points at
/// the enum field itself.
fn remove_matching_string_value(
    value: &mut TomlValue,
    base_path: &str,
    needle: &str,
) -> Option<(String, TomlValue)> {
    let table = value.as_table_mut()?;
    let keys = table.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let child_path = format!("{base_path}.{key}");
        let child = table.get_mut(&key)?;
        if child.as_str() == Some(needle) {
            let removed = table.remove(&key)?;
            return Some((child_path, removed));
        }
        if child.is_table()
            && let Some(removed) = remove_matching_string_value(child, &child_path, needle)
        {
            return Some(removed);
        }
    }
    None
}
