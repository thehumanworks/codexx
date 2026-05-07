use crate::schema::config_schema;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use toml::Value as TomlValue;

const SCHEMA_COMPOSITION_KEYS: [&str; 3] = ["allOf", "anyOf", "oneOf"];

/// String enum choices collected from a schema node and anything it references.
///
/// `allows_non_string` matters for untagged/union schemas such as `bool | table`:
/// if a schema accepts a non-string shape, this warning pass should not report a
/// non-string TOML value as an invalid enum.
#[derive(Default)]
struct EnumChoices {
    allowed_strings: BTreeSet<String>,
    allows_non_string: bool,
}

/// One concrete location in the TOML value being compared with the schema.
enum PathElement {
    /// A table key, including keys from `additionalProperties` maps.
    Key(String),
    /// An array element when the schema has an `items` child.
    Index(usize),
}

/// Return best-effort startup warnings for raw TOML values that look like invalid enums.
///
/// `DefaultOnError` is responsible for keeping config deserialization lenient.
/// This pass is intentionally advisory: it walks the final merged TOML value
/// against the generated config schema and reports enum-looking mismatches
/// without changing the TOML that will be deserialized.
pub(crate) fn enum_value_warnings(value: &TomlValue) -> Vec<String> {
    // Startup warnings should never make config loading fail. If schema
    // generation or traversal panics, the typed config load still proceeds.
    catch_unwind(AssertUnwindSafe(|| {
        match serde_json::to_value(config_schema()) {
            Ok(schema) => {
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
            }
            Err(_) => Vec::new(),
        }
    }))
    .unwrap_or_default()
}

/// Walk a TOML value and a schema node together and append enum warnings.
///
/// The traversal intentionally follows the schema shapes emitted by
/// `config_schema`: object `properties`, map-like `additionalProperties`, array
/// `items`, `$ref`, and schema composition keywords. It never validates the full
/// shape. Serde remains the source of truth for real config parsing.
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

    // Warn only when this schema node has string enum choices. Non-enum shape
    // mismatches are left alone so this pass stays small and advisory.
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

    // Follow references and composition at the same TOML path. Schemars often
    // emits enum fields as `allOf: [{ "$ref": ... }]` when defaults or
    // attributes are present on the field.
    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_warnings(value, definition, definitions, path, ref_stack, warnings);
        ref_stack.pop();
    }

    for key in SCHEMA_COMPOSITION_KEYS {
        let Some(child_schemas) = schema.get(key).and_then(JsonValue::as_array) else {
            continue;
        };
        for child_schema in child_schemas {
            collect_warnings(value, child_schema, definitions, path, ref_stack, warnings);
        }
    }

    // For ordinary tables, recurse only into keys present in both the TOML and
    // the schema. Unknown keys are not part of enum warning collection.
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

    // Dynamic maps such as profiles and MCP servers are represented by
    // `additionalProperties`; every unmatched TOML key uses the same child
    // schema.
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

    // Arrays are rare for enum config today, but following `items` keeps the
    // schema walk honest if a future list contains enum-valued entries.
    let items = schema.get("items");
    if let (Some(array), Some(items)) = (value.as_array(), items) {
        for (index, child_value) in array.iter().enumerate() {
            path.push(PathElement::Index(index));
            collect_warnings(child_value, items, definitions, path, ref_stack, warnings);
            path.pop();
        }
    }
}

/// Collect string enum values reachable from one schema node.
///
/// This mirrors the traversal used by `collect_warnings` at a single location:
/// referenced schemas and union/composition children all contribute choices for
/// the same TOML value.
fn collect_enum_choices(
    schema: &JsonValue,
    definitions: Option<&JsonMap<String, JsonValue>>,
    ref_stack: &mut Vec<String>,
    choices: &mut EnumChoices,
) {
    choices.allowed_strings.extend(
        schema
            .get("enum")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(JsonValue::as_str)
            .map(str::to_string),
    );

    choices.allows_non_string |= match schema.get("type") {
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
    };

    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_enum_choices(definition, definitions, ref_stack, choices);
        ref_stack.pop();
    }

    for key in SCHEMA_COMPOSITION_KEYS {
        let Some(child_schemas) = schema.get(key).and_then(JsonValue::as_array) else {
            continue;
        };
        for child_schema in child_schemas {
            collect_enum_choices(child_schema, definitions, ref_stack, choices);
        }
    }
}

/// Resolve an internal `#/definitions/...` reference while avoiding cycles.
///
/// When this returns `Some`, the resolved name has been pushed onto `ref_stack`;
/// callers must pop after they finish walking that definition.
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
        None
    } else {
        let definition = definitions?.get(name)?;
        ref_stack.push(name.to_string());
        Some(definition)
    }
}

/// Render a warning path using TOML-style dotted keys where possible.
fn display_path(path: &[PathElement]) -> String {
    if path.is_empty() {
        "<root>".to_string()
    } else {
        let mut output = String::new();
        for element in path {
            match element {
                PathElement::Key(key) => {
                    if !output.is_empty() {
                        output.push('.');
                    }
                    if !key.is_empty()
                        && key
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                    {
                        output.push_str(key);
                    } else {
                        let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
                        output.push('"');
                        output.push_str(&escaped);
                        output.push('"');
                    }
                }
                PathElement::Index(index) => {
                    output.push_str(&format!("[{index}]"));
                }
            }
        }
        output
    }
}
