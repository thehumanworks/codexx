use crate::ToolConfig;
use codex_protocol::config_types::Personality;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// API request payload for a single model turn.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub input: Vec<ResponseItem>,
    pub tools: Vec<ToolSpec>,
    pub parallel_tool_calls: bool,
    pub base_instructions: BaseInstructions,
    pub personality: Option<Personality>,
    pub output_schema: Option<Value>,
    pub output_schema_strict: bool,
}

/// Retry-stable prompt settings used to rebuild a [`Prompt`] after tool calls or stream retries.
#[derive(Debug, Clone)]
pub struct PromptConfig {
    pub base_instructions: BaseInstructions,
    pub personality: Option<Personality>,
    pub output_schema: Option<Value>,
    pub output_schema_strict: bool,
}

impl Prompt {
    pub fn get_formatted_input(&self) -> Vec<ResponseItem> {
        let mut input = self.input.clone();
        let is_freeform_apply_patch_tool_present = self.tools.iter().any(|tool| match tool {
            ToolSpec::Freeform(f) => f.name == "apply_patch",
            _ => false,
        });
        if is_freeform_apply_patch_tool_present {
            reserialize_shell_outputs(&mut input);
        }

        input
    }
}

impl PromptConfig {
    pub fn build_prompt(&self, input: Vec<ResponseItem>, tool_config: &ToolConfig) -> Prompt {
        Prompt {
            input,
            tools: tool_config.tools.clone(),
            parallel_tool_calls: tool_config.parallel_tool_calls,
            base_instructions: self.base_instructions.clone(),
            personality: self.personality,
            output_schema: self.output_schema.clone(),
            output_schema_strict: self.output_schema_strict,
        }
    }
}

impl Default for Prompt {
    fn default() -> Self {
        Self {
            input: Vec::new(),
            tools: Vec::new(),
            parallel_tool_calls: false,
            base_instructions: BaseInstructions::default(),
            personality: None,
            output_schema: None,
            output_schema_strict: true,
        }
    }
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            base_instructions: BaseInstructions::default(),
            personality: None,
            output_schema: None,
            output_schema_strict: true,
        }
    }
}

fn reserialize_shell_outputs(items: &mut [ResponseItem]) {
    let mut shell_call_ids: HashSet<String> = HashSet::new();

    items.iter_mut().for_each(|item| match item {
        ResponseItem::LocalShellCall { call_id, id, .. } => {
            if let Some(identifier) = call_id.clone().or_else(|| id.clone()) {
                shell_call_ids.insert(identifier);
            }
        }
        ResponseItem::CustomToolCall {
            id: _,
            status: _,
            call_id,
            name,
            input: _,
        } => {
            if name == "apply_patch" {
                shell_call_ids.insert(call_id.clone());
            }
        }
        ResponseItem::FunctionCall { name, call_id, .. }
            if is_shell_tool_name(name) || name == "apply_patch" =>
        {
            shell_call_ids.insert(call_id.clone());
        }
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        }
        | ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            if shell_call_ids.remove(call_id)
                && let Some(structured) = output
                    .text_content()
                    .and_then(parse_structured_shell_output)
            {
                output.body = FunctionCallOutputBody::Text(structured);
            }
        }
        _ => {}
    });
}

fn is_shell_tool_name(name: &str) -> bool {
    matches!(name, "shell" | "container.exec")
}

#[derive(Deserialize)]
struct ExecOutputJson {
    output: String,
    metadata: ExecOutputMetadataJson,
}

#[derive(Deserialize)]
struct ExecOutputMetadataJson {
    exit_code: i32,
    duration_seconds: f32,
}

fn parse_structured_shell_output(raw: &str) -> Option<String> {
    let parsed: ExecOutputJson = serde_json::from_str(raw).ok()?;
    Some(build_structured_output(&parsed))
}

fn build_structured_output(parsed: &ExecOutputJson) -> String {
    let mut sections = Vec::new();
    let exit_code = parsed.metadata.exit_code;
    sections.push(format!("Exit code: {exit_code}"));
    let duration_seconds = parsed.metadata.duration_seconds;
    sections.push(format!("Wall time: {duration_seconds} seconds"));

    let mut output = parsed.output.clone();
    if let Some((stripped, total_lines)) = strip_total_output_header(&parsed.output) {
        sections.push(format!("Total output lines: {total_lines}"));
        output = stripped.to_string();
    }

    sections.push("Output:".to_string());
    sections.push(output);
    sections.join("\n")
}

fn strip_total_output_header(output: &str) -> Option<(&str, u32)> {
    let after_prefix = output.strip_prefix("Total output lines: ")?;
    let (total_segment, remainder) = after_prefix.split_once('\n')?;
    let total_lines = total_segment.parse::<u32>().ok()?;
    let remainder = remainder.strip_prefix('\n').unwrap_or(remainder);
    Some((remainder, total_lines))
}

#[cfg(test)]
mod tests {
    use super::Prompt;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::models::ResponseItem;
    use codex_tools::FreeformTool;
    use codex_tools::FreeformToolFormat;
    use codex_tools::ToolSpec;
    use pretty_assertions::assert_eq;

    #[test]
    fn reserializes_shell_outputs_for_function_and_custom_tool_calls() {
        let raw_output = r#"{"output":"hello","metadata":{"exit_code":0,"duration_seconds":0.5}}"#;
        let expected_output = "Exit code: 0\nWall time: 0.5 seconds\nOutput:\nhello";
        let prompt = Prompt {
            input: vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call-1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call-1".to_string(),
                    output: FunctionCallOutputPayload::from_text(raw_output.to_string()),
                },
                ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: "call-2".to_string(),
                    name: "apply_patch".to_string(),
                    input: "*** Begin Patch".to_string(),
                },
                ResponseItem::CustomToolCallOutput {
                    call_id: "call-2".to_string(),
                    name: None,
                    output: FunctionCallOutputPayload::from_text(raw_output.to_string()),
                },
            ],
            tools: vec![ToolSpec::Freeform(FreeformTool {
                name: "apply_patch".to_string(),
                description: "patch".to_string(),
                format: FreeformToolFormat {
                    r#type: "grammar".to_string(),
                    syntax: "lark".to_string(),
                    definition: "patch".to_string(),
                },
            })],
            ..Prompt::default()
        };

        assert_eq!(
            prompt.get_formatted_input(),
            vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{}".to_string(),
                    call_id: "call-1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call-1".to_string(),
                    output: FunctionCallOutputPayload::from_text(expected_output.to_string()),
                },
                ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: "call-2".to_string(),
                    name: "apply_patch".to_string(),
                    input: "*** Begin Patch".to_string(),
                },
                ResponseItem::CustomToolCallOutput {
                    call_id: "call-2".to_string(),
                    name: None,
                    output: FunctionCallOutputPayload::from_text(expected_output.to_string()),
                },
            ]
        );
    }
}
