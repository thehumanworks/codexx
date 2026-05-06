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
        // serde_path_to_error gives us the exact config path that failed, so
        // we can remove the offending TOML value without making invalid enum
        // state representable inside ConfigToml or its nested structs.
        match serde_path_to_error::deserialize(value.clone()) {
            Ok(parsed) => return Ok((value, parsed, warnings)),
            Err(err) => {
                let path = err.path().to_string();
                let toml_error = err.into_inner();
                if !is_unknown_variant_error(&toml_error) {
                    return Err(toml_error);
                }

                let Some(invalid_value) = remove_value_at_path(&mut value, &path) else {
                    return Err(toml_error);
                };
                warnings.push(format!(
                    "Ignoring invalid config value at {path}: {invalid_value}"
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

/// Removes the failed TOML key identified by serde_path_to_error.
///
/// Config enum fields live in tables rather than arrays, so the path traversal
/// intentionally follows table keys only. If the path cannot be removed, the
/// caller falls back to returning the original deserialization error.
fn remove_value_at_path(value: &mut TomlValue, path: &str) -> Option<TomlValue> {
    let mut parts = path.split('.').peekable();
    let mut current = value;

    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return current.as_table_mut()?.remove(part);
        }
        current = current.as_table_mut()?.get_mut(part)?;
    }

    None
}
