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

impl EnumChoices {
    fn has_string_enum(&self) -> bool {
        !self.allowed_strings.is_empty()
    }

    fn accepts(&self, value: &TomlValue) -> bool {
        if let Some(value) = value.as_str() {
            self.allowed_strings.contains(value)
        } else {
            self.allows_non_string
        }
    }
}

/// One concrete location in the TOML value being compared with the schema.
enum PathElement {
    /// A table key, including keys from `additionalProperties` maps.
    Key(String),
    /// An array element when the schema has an `items` child.
    Index(usize),
}

enum CurrentNodeWarning {
    Check,
    Skip,
}

/// Return best-effort warnings for raw TOML values that look like invalid enums.
///
/// This pass is intentionally advisory: it walks the final merged TOML value
/// against the generated config schema and reports enum-looking mismatches
/// without changing the TOML that will be deserialized. Narrow typed-config
/// fallbacks keep the reported enum leaves non-blocking for startup loading;
/// config write paths may use the same signal to reject newly provided invalid
/// enum values.
pub fn invalid_enum_warnings(value: &TomlValue) -> Vec<String> {
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
                    CurrentNodeWarning::Check,
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
    current_node_warning: CurrentNodeWarning,
) {
    let mut choices = EnumChoices::default();
    collect_enum_choices(schema, definitions, ref_stack, &mut choices);

    // Warn only when this schema node has string enum choices. Non-enum shape
    // mismatches are left alone so this pass stays small and advisory.
    if matches!(current_node_warning, CurrentNodeWarning::Check)
        && choices.has_string_enum()
        && !choices.accepts(value)
    {
        let warning = format!(
            "Ignoring invalid config value at {}: {value}",
            display_path(path)
        );
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }

    // Follow references and composition at the same TOML path so nested object
    // schemas are still traversed. Current-path warnings are skipped here
    // because the enum choices above already aggregate refs and composition
    // children; warning inside each oneOf branch would reject valid sibling
    // enum variants.
    if let Some(definition) = resolve_definition(schema, definitions, ref_stack) {
        collect_warnings(
            value,
            definition,
            definitions,
            path,
            ref_stack,
            warnings,
            CurrentNodeWarning::Skip,
        );
        ref_stack.pop();
    }

    for key in SCHEMA_COMPOSITION_KEYS {
        let Some(child_schemas) = schema.get(key).and_then(JsonValue::as_array) else {
            continue;
        };

        // Tagged object unions put the enum on a child property such as
        // `type`. First find enum properties accepted by at least one sibling
        // branch; invalid values match no branch here, so the fallback below
        // still walks every branch and reports the bad leaf.
        let mut matched_enum_properties: BTreeSet<&str> = BTreeSet::new();
        if key != "allOf"
            && let Some(table) = value.as_table()
        {
            for child_schema in child_schemas {
                let Some(properties) = child_schema
                    .get("properties")
                    .and_then(JsonValue::as_object)
                else {
                    continue;
                };
                for (property_name, property_schema) in properties {
                    let Some(child_value) = table.get(property_name) else {
                        continue;
                    };

                    let mut property_choices = EnumChoices::default();
                    let mut choice_ref_stack = ref_stack.clone();
                    collect_enum_choices(
                        property_schema,
                        definitions,
                        &mut choice_ref_stack,
                        &mut property_choices,
                    );
                    if property_choices.has_string_enum() && property_choices.accepts(child_value) {
                        matched_enum_properties.insert(property_name.as_str());
                    }
                }
            }
        }

        let mut matching_child_schemas = Vec::new();
        for child_schema in child_schemas {
            let mut child_matches_value = true;

            if !matched_enum_properties.is_empty()
                && let (Some(table), Some(properties)) = (
                    value.as_table(),
                    child_schema
                        .get("properties")
                        .and_then(JsonValue::as_object),
                )
            {
                for property_name in &matched_enum_properties {
                    let (Some(child_value), Some(property_schema)) =
                        (table.get(*property_name), properties.get(*property_name))
                    else {
                        continue;
                    };

                    let mut property_choices = EnumChoices::default();
                    let mut choice_ref_stack = ref_stack.clone();
                    collect_enum_choices(
                        property_schema,
                        definitions,
                        &mut choice_ref_stack,
                        &mut property_choices,
                    );
                    if property_choices.has_string_enum() && !property_choices.accepts(child_value)
                    {
                        child_matches_value = false;
                        break;
                    }
                }
            }

            if child_matches_value {
                matching_child_schemas.push(child_schema);
            }
        }

        let child_schemas_to_walk = if matching_child_schemas.is_empty() {
            child_schemas.iter().collect()
        } else {
            matching_child_schemas
        };

        for child_schema in child_schemas_to_walk {
            collect_warnings(
                value,
                child_schema,
                definitions,
                path,
                ref_stack,
                warnings,
                CurrentNodeWarning::Skip,
            );
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
                CurrentNodeWarning::Check,
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
                CurrentNodeWarning::Check,
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
            collect_warnings(
                child_value,
                items,
                definitions,
                path,
                ref_stack,
                warnings,
                CurrentNodeWarning::Check,
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;

    fn warning_set(contents: &str) -> BTreeSet<String> {
        let value = toml::from_str::<TomlValue>(contents).expect("config should parse");
        invalid_enum_warnings(&value).into_iter().collect()
    }

    #[test]
    fn enum_warning_walk_follows_refs_composition_and_dynamic_maps() {
        let contents = r#"
sandbox_mode = "hyperdrive"

[tui]
notification_method = "shout"
notification_condition = "whenever"

[profiles."alpha.beta"]
sandbox_mode = "moonwalk"

[mcp_servers.local]
command = "server"
default_tools_approval_mode = "ship-it"

[mcp_servers.local.tools."danger.tool"]
approval_mode = "yolo"
"#;
        let value = toml::from_str::<TomlValue>(contents).expect("config should parse");
        let original = value.clone();
        let warnings = invalid_enum_warnings(&value).into_iter().collect();
        let expected_warnings = BTreeSet::from([
            "Ignoring invalid config value at mcp_servers.local.default_tools_approval_mode: \
             \"ship-it\""
                .to_string(),
            "Ignoring invalid config value at mcp_servers.local.tools.\"danger.tool\".\
             approval_mode: \"yolo\""
                .to_string(),
            "Ignoring invalid config value at profiles.\"alpha.beta\".sandbox_mode: \"moonwalk\""
                .to_string(),
            "Ignoring invalid config value at sandbox_mode: \"hyperdrive\"".to_string(),
            "Ignoring invalid config value at tui.notification_condition: \"whenever\"".to_string(),
            "Ignoring invalid config value at tui.notification_method: \"shout\"".to_string(),
        ]);

        assert_eq!((value, warnings), (original, expected_warnings));
    }

    #[test]
    fn valid_one_of_enum_values_do_not_warn_from_sibling_branches() {
        let warnings = warning_set(
            r#"
[tui]
notification_condition = "always"
"#,
        );

        assert_eq!(warnings, BTreeSet::new());
    }

    #[test]
    fn tagged_union_sibling_variants_do_not_warn() {
        let warnings = warning_set(
            r#"
[hooks]

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "python3 /tmp/user-prompt.py"
"#,
        );

        assert_eq!(warnings, BTreeSet::new());
    }

    #[test]
    fn invalid_tagged_union_variant_warns_at_tag_property() {
        let warnings = warning_set(
            r#"
[hooks]

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "python"
command = "python3 /tmp/user-prompt.py"
"#,
        );
        let expected_warnings = BTreeSet::from([String::from(
            "Ignoring invalid config value at hooks.UserPromptSubmit[0].hooks[0].type: \"python\"",
        )]);

        assert_eq!(warnings, expected_warnings);
    }

    #[test]
    fn non_string_union_branches_do_not_warn() {
        let warnings = warning_set(
            r#"
[tools]
web_search = true

[tui]
notifications = ["notify-send"]
"#,
        );

        assert_eq!(warnings, BTreeSet::new());
    }
}
