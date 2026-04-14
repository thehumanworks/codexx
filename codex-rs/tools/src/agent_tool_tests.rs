use super::*;
use crate::JsonSchemaPrimitiveType;
use crate::JsonSchemaType;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use pretty_assertions::assert_eq;
use serde_json::json;

fn model_preset(id: &str, show_in_picker: bool) -> ModelPreset {
    ModelPreset {
        id: id.to_string(),
        model: format!("{id}-model"),
        display_name: format!("{id} display"),
        description: format!("{id} description"),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "Balanced".to_string(),
        }],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        is_default: false,
        upgrade: None,
        show_in_picker,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: Vec::new(),
    }
}

#[test]
fn spawn_agent_tool_v2_requires_task_name_and_lists_visible_models() {
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: &[
            model_preset("visible", /*show_in_picker*/ true),
            model_preset("hidden", /*show_in_picker*/ false),
        ],
        agent_type_description: "role help".to_string(),
        hide_agent_type_model_reasoning: false,
        include_usage_hint: true,
        usage_hint_text: None,
        max_concurrent_threads_per_session: Some(4),
    });

    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = tool
    else {
        panic!("spawn_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");
    assert!(description.contains("Spawns an agent to work on the specified task."));
    assert!(description.contains("The spawned agent will have the same tools as you"));
    assert!(description.contains("`max_concurrent_threads_per_session = 4`"));
    assert!(description.contains(SPAWN_AGENT_INHERITED_MODEL_GUIDANCE));
    assert!(
        description
            .contains("Available model overrides (optional; inherited parent model is preferred):")
    );
    assert!(description.contains("visible display (`visible-model`)"));
    assert!(!description.contains("hidden display (`hidden-model`)"));
    assert!(properties.contains_key("task_name"));
    assert!(properties.contains_key("message"));
    assert!(properties.contains_key("fork_turns"));
    assert!(properties.contains_key("fork_context"));
    assert!(properties.contains_key("model_fallback_list"));
    assert!(!properties.contains_key("items"));
    assert_eq!(
        properties.get("agent_type"),
        Some(&JsonSchema::string(Some("role help".to_string())))
    );
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(spawn_agent_model_override_description_v2().as_str())
    );
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["task_name".to_string(), "message".to_string()])
    );
    let Some(model_fallback_list) = properties.get("model_fallback_list") else {
        panic!("spawn_agent v2 should define model_fallback_list as an array");
    };
    assert_eq!(
        model_fallback_list.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array))
    );
    let model_fallback_items = model_fallback_list
        .items
        .as_ref()
        .expect("model_fallback_list should define item schema");
    assert_eq!(
        model_fallback_items.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let model_fallback_item_properties = model_fallback_items
        .properties
        .as_ref()
        .expect("spawn_agent v2 model_fallback_list items should be objects");
    let model_fallback_item_required = model_fallback_items
        .required
        .as_ref()
        .expect("model_fallback_list items should require model");
    if model_fallback_items.additional_properties != Some(false.into()) {
        panic!("spawn_agent v2 model_fallback_list items should be objects");
    }
    assert_eq!(
        model_fallback_item_properties.get("model"),
        Some(&JsonSchema::string(Some(
            "Model to try. Must be a model slug from the current model picker list.".to_string(),
        )))
    );
    assert!(model_fallback_item_properties.contains_key("reasoning_effort"));
    assert_eq!(model_fallback_item_required, &vec!["model".to_string()]);
    assert_eq!(
        output_schema.expect("spawn_agent output schema")["required"],
        json!(["task_name", "nickname"])
    );
}

#[test]
fn spawn_agent_tool_v1_includes_model_fallback_list() {
    let tool = create_spawn_agent_tool_v1(SpawnAgentToolOptions {
        available_models: &[],
        agent_type_description: "role help".to_string(),
        hide_agent_type_model_reasoning: false,
        include_usage_hint: true,
        usage_hint_text: None,
        max_concurrent_threads_per_session: None,
    });

    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = tool else {
        panic!("spawn_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");
    let Some(model_fallback_list) = properties.get("model_fallback_list") else {
        panic!("spawn_agent v1 should define model_fallback_list as an array");
    };
    assert_eq!(
        model_fallback_list.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Array))
    );
    assert!(properties.contains_key("model_fallback_list"));
    assert!(properties.contains_key("fork_context"));
    assert!(!properties.contains_key("fork_turns"));
    assert_eq!(
        properties
            .get("model")
            .and_then(|schema| schema.description.as_deref()),
        Some(spawn_agent_model_override_description_v1().as_str())
    );
    assert_eq!(
        properties.get("reasoning_effort"),
        Some(&JsonSchema::string(Some(
            "Optional reasoning effort override for the new agent. Replaces the inherited reasoning effort only when fork_context is false; forked children always inherit the parent reasoning effort."
                .to_string(),
        )))
    );
}

#[test]
fn spawn_agent_tool_v2_documents_that_forked_children_ignore_model_overrides() {
    let tool = create_spawn_agent_tool_v2(SpawnAgentToolOptions {
        available_models: &[],
        agent_type_description: "role help".to_string(),
        hide_agent_type_model_reasoning: false,
        include_usage_hint: true,
        usage_hint_text: None,
        max_concurrent_threads_per_session: None,
    });

    let ToolSpec::Function(ResponsesApiTool { parameters, .. }) = tool else {
        panic!("spawn_agent should be a function tool");
    };
    let properties = parameters
        .properties
        .as_ref()
        .expect("spawn_agent should use object params");

    assert_eq!(
        properties.get("model"),
        Some(&JsonSchema::string(Some(
            spawn_agent_model_override_description_v2(),
        )))
    );
    assert_eq!(
        properties.get("reasoning_effort"),
        Some(&JsonSchema::string(Some(
            "Optional reasoning effort override for the new agent. Replaces the inherited reasoning effort only when fork_turns is `none`; forked children always inherit the parent reasoning effort."
                .to_string(),
        )))
    );
}

#[test]
fn send_message_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_send_message_tool()
    else {
        panic!("send_message should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("send_message should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert!(!properties.contains_key("interrupt"));
    assert!(!properties.contains_key("items"));
    assert_eq!(
        properties
            .get("target")
            .and_then(|schema| schema.description.as_deref()),
        Some("Relative or canonical task name to message (from spawn_agent).")
    );
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["target".to_string(), "message".to_string()])
    );
    assert_eq!(output_schema, None);
}

#[test]
fn followup_task_tool_requires_message_and_has_no_output_schema() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_followup_task_tool()
    else {
        panic!("followup_task should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("followup_task should use object params");
    assert!(properties.contains_key("target"));
    assert!(properties.contains_key("message"));
    assert!(!properties.contains_key("items"));
    assert_eq!(
        parameters.required.as_ref(),
        Some(&vec!["target".to_string(), "message".to_string()])
    );
    assert_eq!(output_schema, None);
}

#[test]
fn wait_agent_tool_v2_uses_timeout_only_summary_output() {
    let ToolSpec::Function(ResponsesApiTool {
        description,
        parameters,
        output_schema,
        ..
    }) = create_wait_agent_tool_v2(WaitAgentTimeoutOptions {
        default_timeout_ms: 30_000,
        min_timeout_ms: 10_000,
        max_timeout_ms: 3_600_000,
    })
    else {
        panic!("wait_agent should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("wait_agent should use object params");
    assert!(!properties.contains_key("targets"));
    assert!(properties.contains_key("timeout_ms"));
    assert!(description.contains(
        "Does not return the content; returns either a summary of which agents have updates (if any)"
    ));
    assert_eq!(
        properties
            .get("timeout_ms")
            .and_then(|schema| schema.description.as_deref()),
        Some("Optional timeout in milliseconds. Defaults to 30000, min 10000, max 3600000.")
    );
    assert_eq!(parameters.required.as_ref(), None);
    assert_eq!(
        output_schema.expect("wait output schema")["properties"]["message"]["description"],
        json!("Brief wait summary without the agent's final content.")
    );
}

#[test]
fn list_agents_tool_includes_path_prefix_and_agent_fields() {
    let ToolSpec::Function(ResponsesApiTool {
        parameters,
        output_schema,
        ..
    }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };
    assert_eq!(
        parameters.schema_type,
        Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Object))
    );
    let properties = parameters
        .properties
        .as_ref()
        .expect("list_agents should use object params");
    assert!(properties.contains_key("path_prefix"));
    assert_eq!(
        properties
            .get("path_prefix")
            .and_then(|schema| schema.description.as_deref()),
        Some(
            "Optional task-path prefix (not ending with trailing slash). Accepts the same relative or absolute task-path syntax."
        )
    );
    assert_eq!(
        output_schema.expect("list_agents output schema")["properties"]["agents"]["items"]["required"],
        json!(["agent_name", "agent_status", "last_task_message"])
    );
}

#[test]
fn list_agents_tool_status_schema_includes_interrupted() {
    let ToolSpec::Function(ResponsesApiTool { output_schema, .. }) = create_list_agents_tool()
    else {
        panic!("list_agents should be a function tool");
    };

    assert_eq!(
        output_schema.expect("list_agents output schema")["properties"]["agents"]["items"]["properties"]
            ["agent_status"]["allOf"][0]["oneOf"][0]["enum"],
        json!([
            "pending_init",
            "running",
            "interrupted",
            "shutdown",
            "not_found"
        ])
    );
}
