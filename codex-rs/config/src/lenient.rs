use crate::schema::config_schema;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use toml::Value as TomlValue;

#[derive(Default)]
struct EnumChoices {
    allowed_strings: BTreeSet<String>,
    allows_non_string: bool,
}

enum PathElement {
    Key(String),
    Index(usize),
}

/// Return best-effort startup warnings for raw TOML values that look like invalid enums.
///
/// `DefaultOnError` is responsible for keeping config deserialization lenient.
/// This pass is intentionally advisory: it walks the final merged TOML value
/// against the generated config schema and reports enum-looking mismatches
/// without changing the TOML that will be deserialized.
pub(crate) fn enum_value_warnings(value: &TomlValue) -> Vec<String> {
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(schema) = serde_json::to_value(config_schema()) else {
            return Vec::new();
        };
        let definitions = schema.get("definitions").and_then(JsonValue::as_object);
        let mut path = Vec::new();
        let mut ref_stack = Vec::new();
        let mut warnings = Vec::new();

        collect_warnings(
            value,
            &schema,
            definitions,
            &mut path,
            &mut ref_stack,
            &mut warnings,
        );

        warnings
    }))
    .unwrap_or_default()
}

fn collect_warnings(
    value: &TomlValue,
    schema: &JsonValue,
    definitions: Option<&JsonMap<String, JsonValue>>,
    path: &mut Vec<PathElement>,
    ref_stack: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let mut choices = EnumChoices::default();
    collect_enum_choices(schema, definitions, ref_stack, &mut choices);
    let invalid_enum_value = if let Some(value) = value.as_str() {
        !choices.allowed_strings.contains(value)
    } else {
        !choices.allows_non_string
    };
    if !choices.allowed_strings.is_empty() && invalid_enum_value {
        let warning = format!(
            "Ignoring invalid config value at {}: {value}",
            display_path(path)
        );
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }

    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_warnings(value, definition, definitions, path, ref_stack, warnings);
        ref_stack.pop();
    }

    for child_schema in schema_composition_children(schema) {
        collect_warnings(value, child_schema, definitions, path, ref_stack, warnings);
    }

    let properties = schema.get("properties").and_then(JsonValue::as_object);
    if let (Some(table), Some(properties)) = (value.as_table(), properties) {
        for (key, child_schema) in properties {
            let Some(child_value) = table.get(key) else {
                continue;
            };
            path.push(PathElement::Key(key.clone()));
            collect_warnings(
                child_value,
                child_schema,
                definitions,
                path,
                ref_stack,
                warnings,
            );
            path.pop();
        }
    }

    let additional_properties = schema
        .get("additionalProperties")
        .filter(|additional_properties| additional_properties.is_object());
    if let (Some(table), Some(additional_properties)) = (value.as_table(), additional_properties) {
        for (key, child_value) in table {
            if properties.is_some_and(|properties| properties.contains_key(key)) {
                continue;
            }
            path.push(PathElement::Key(key.clone()));
            collect_warnings(
                child_value,
                additional_properties,
                definitions,
                path,
                ref_stack,
                warnings,
            );
            path.pop();
        }
    }

    let items = schema.get("items");
    if let (Some(array), Some(items)) = (value.as_array(), items) {
        for (index, child_value) in array.iter().enumerate() {
            path.push(PathElement::Index(index));
            collect_warnings(child_value, items, definitions, path, ref_stack, warnings);
            path.pop();
        }
    }
}

fn collect_enum_choices(
    schema: &JsonValue,
    definitions: Option<&JsonMap<String, JsonValue>>,
    ref_stack: &mut Vec<String>,
    choices: &mut EnumChoices,
) {
    choices
        .allowed_strings
        .extend(string_enum_values(schema).map(str::to_string));
    choices.allows_non_string |= schema_allows_non_string(schema);

    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_enum_choices(definition, definitions, ref_stack, choices);
        ref_stack.pop();
    }

    for child_schema in schema_composition_children(schema) {
        collect_enum_choices(child_schema, definitions, ref_stack, choices);
    }
}

fn string_enum_values(schema: &JsonValue) -> impl Iterator<Item = &str> {
    schema
        .get("enum")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(JsonValue::as_str)
}

fn schema_allows_non_string(schema: &JsonValue) -> bool {
    match schema.get("type") {
        Some(JsonValue::String(value)) => value != "string" && value != "null",
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(JsonValue::as_str)
            .any(|value| value != "string" && value != "null"),
        _ => {
            schema.get("properties").is_some()
                || schema.get("additionalProperties").is_some()
                || schema.get("items").is_some()
        }
    }
}

fn resolve_definition<'a>(
    schema: &JsonValue,
    definitions: Option<&'a JsonMap<String, JsonValue>>,
    ref_stack: &mut Vec<String>,
) -> Option<&'a JsonValue> {
    let name = schema
        .get("$ref")?
        .as_str()?
        .strip_prefix("#/definitions/")?;
    if ref_stack.iter().any(|entry| entry == name) {
        return None;
    }
    let definition = definitions?.get(name)?;
    ref_stack.push(name.to_string());
    Some(definition)
}

fn schema_composition_children(schema: &JsonValue) -> impl Iterator<Item = &JsonValue> {
    ["allOf", "anyOf", "oneOf"]
        .into_iter()
        .filter_map(|key| schema.get(key).and_then(JsonValue::as_array))
        .flatten()
}

fn display_path(path: &[PathElement]) -> String {
    if path.is_empty() {
        return "<root>".to_string();
    }

    let mut output = String::new();
    for element in path {
        match element {
            PathElement::Key(key) => {
                if !output.is_empty() {
                    output.push('.');
                }
                output.push_str(&display_key(key));
            }
            PathElement::Index(index) => {
                output.push_str(&format!("[{index}]"));
            }
        }
    }
    output
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
