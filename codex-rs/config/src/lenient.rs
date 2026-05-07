use crate::config_toml::ConfigToml;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use toml::Value as TomlValue;

static ENUM_FIELD_SPECS: OnceLock<Vec<EnumFieldSpec>> = OnceLock::new();

#[derive(Clone, Debug)]
struct EnumFieldSpec {
    path: Vec<PathSegment>,
    allowed_values: BTreeSet<String>,
    allows_non_string: bool,
}

/// One segment in a known enum-valued config path discovered from JSON Schema.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum PathSegment {
    /// Match this literal table key.
    Key(String),
    /// Expand across every table child at this point, such as `profiles.*`.
    AnyTable,
}

#[derive(Default)]
struct ScalarChoices {
    allowed_strings: BTreeSet<String>,
    allows_non_string: bool,
}

/// Deserializes config while turning known invalid enum-valued settings into warnings.
///
/// The config structs use `serde_with::DefaultOnError` on their enum fields so
/// an invalid field can default instead of rejecting the whole file. This helper
/// uses the same generated JSON Schema that backs `config.schema.json` to find
/// string enum-valued leaves. It then does a best-effort pass over the raw TOML:
/// invalid enum leaves are deleted from the sanitized TOML and reported as
/// startup warnings, while full shape/type validation stays with Serde.
pub(crate) fn deserialize_with_enum_warnings(
    value: TomlValue,
) -> Result<(TomlValue, ConfigToml, Vec<String>), toml::de::Error> {
    let mut sanitized = value;
    let mut warnings = Vec::new();
    for spec in enum_field_specs() {
        for path in matching_paths(&sanitized, &spec.path) {
            remove_if_invalid(&mut sanitized, &path, spec, &mut warnings);
        }
    }
    let parsed = sanitized.clone().try_into::<ConfigToml>()?;
    Ok((sanitized, parsed, warnings))
}

fn enum_field_specs() -> &'static [EnumFieldSpec] {
    ENUM_FIELD_SPECS.get_or_init(build_enum_field_specs)
}

fn build_enum_field_specs() -> Vec<EnumFieldSpec> {
    let schema = serde_json::to_value(crate::schema::config_schema())
        .expect("generated config schema should serialize");
    let definitions = schema.get("definitions").and_then(JsonValue::as_object);
    let mut choices_by_path = BTreeMap::<Vec<PathSegment>, ScalarChoices>::new();
    let mut path = Vec::new();
    let mut ref_stack = Vec::new();

    collect_enum_fields(
        &schema,
        definitions,
        &mut path,
        &mut choices_by_path,
        &mut ref_stack,
    );

    choices_by_path
        .into_iter()
        .filter_map(|(path, choices)| {
            if choices.allowed_strings.is_empty() {
                return None;
            }
            Some(EnumFieldSpec {
                path,
                allowed_values: choices.allowed_strings,
                allows_non_string: choices.allows_non_string,
            })
        })
        .collect()
}

fn collect_enum_fields(
    schema: &JsonValue,
    definitions: Option<&JsonMap<String, JsonValue>>,
    path: &mut Vec<PathSegment>,
    choices_by_path: &mut BTreeMap<Vec<PathSegment>, ScalarChoices>,
    ref_stack: &mut Vec<String>,
) {
    let mut choices = ScalarChoices::default();
    collect_scalar_choices(schema, definitions, &mut choices, ref_stack);
    if !choices.allowed_strings.is_empty() {
        choices_by_path
            .entry(path.clone())
            .or_default()
            .merge(choices);
    }

    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_enum_fields(definition, definitions, path, choices_by_path, ref_stack);
        ref_stack.pop();
    }

    for child in schema_composition_children(schema) {
        collect_enum_fields(child, definitions, path, choices_by_path, ref_stack);
    }

    if let Some(properties) = schema.get("properties").and_then(JsonValue::as_object) {
        for (key, child) in properties {
            path.push(PathSegment::Key(key.clone()));
            collect_enum_fields(child, definitions, path, choices_by_path, ref_stack);
            path.pop();
        }
    }

    let Some(additional_properties) = schema.get("additionalProperties") else {
        return;
    };
    if !additional_properties.is_object() {
        return;
    }
    path.push(PathSegment::AnyTable);
    collect_enum_fields(
        additional_properties,
        definitions,
        path,
        choices_by_path,
        ref_stack,
    );
    path.pop();
}

fn collect_scalar_choices(
    schema: &JsonValue,
    definitions: Option<&JsonMap<String, JsonValue>>,
    choices: &mut ScalarChoices,
    ref_stack: &mut Vec<String>,
) {
    choices
        .allowed_strings
        .extend(string_enum_values(schema).map(str::to_string));
    choices.allows_non_string |= schema_allows_non_string(schema);

    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_scalar_choices(definition, definitions, choices, ref_stack);
        ref_stack.pop();
    }

    for child in schema_composition_children(schema) {
        collect_scalar_choices(child, definitions, choices, ref_stack);
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

/// Removes one known enum leaf when its raw TOML value does not match schema choices.
fn remove_if_invalid(
    value: &mut TomlValue,
    path: &[String],
    spec: &EnumFieldSpec,
    warnings: &mut Vec<String>,
) {
    let Some(raw_value) = value_at_path(value, path) else {
        return;
    };
    if let Some(raw_value) = raw_value.as_str() {
        if spec.allowed_values.contains(raw_value) {
            return;
        }
    } else if spec.allows_non_string {
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
            PathSegment::Key(key) => {
                for path in &mut paths {
                    path.push(key.clone());
                }
            }
            PathSegment::AnyTable => {
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

impl ScalarChoices {
    fn merge(&mut self, other: ScalarChoices) {
        self.allowed_strings.extend(other.allowed_strings);
        self.allows_non_string |= other.allows_non_string;
    }
}
