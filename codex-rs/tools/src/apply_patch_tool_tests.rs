use super::*;
use crate::JsonSchema;
use pretty_assertions::assert_eq;

#[test]
fn freeform_tool_uses_single_environment_grammar_by_default() {
    let ToolSpec::Freeform(tool) = create_apply_patch_freeform_tool(ApplyPatchToolOptions {
        include_environment_id: false,
    }) else {
        panic!("expected freeform tool");
    };

    assert_eq!(tool.name, "apply_patch");
    assert_eq!(tool.format.syntax, "lark");
    assert!(!tool.format.definition.contains("*** Environment ID: "));
}

#[test]
fn freeform_tool_advertises_environment_metadata_in_multi_environment_mode() {
    let ToolSpec::Freeform(tool) = create_apply_patch_freeform_tool(ApplyPatchToolOptions {
        include_environment_id: true,
    }) else {
        panic!("expected freeform tool");
    };

    assert!(tool.format.definition.contains("*** Environment ID: "));
}

#[test]
fn json_tool_omits_environment_id_by_default() {
    let ToolSpec::Function(tool) = create_apply_patch_json_tool(ApplyPatchToolOptions {
        include_environment_id: false,
    }) else {
        panic!("expected function tool");
    };

    let properties = tool
        .parameters
        .properties
        .as_ref()
        .expect("expected properties");
    assert!(properties.contains_key("input"));
    assert!(!properties.contains_key("environment_id"));
    assert_eq!(tool.parameters.required, Some(vec!["input".to_string()]));
}

#[test]
fn json_tool_advertises_environment_id_in_multi_environment_mode() {
    let ToolSpec::Function(tool) = create_apply_patch_json_tool(ApplyPatchToolOptions {
        include_environment_id: true,
    }) else {
        panic!("expected function tool");
    };

    let properties = tool
        .parameters
        .properties
        .as_ref()
        .expect("expected properties");
    assert!(matches!(
        properties.get("environment_id"),
        Some(JsonSchema {
            description: Some(_),
            ..
        })
    ));
    assert_eq!(tool.parameters.required, Some(vec!["input".to_string()]));
}
