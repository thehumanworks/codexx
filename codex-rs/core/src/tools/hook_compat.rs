use serde_json::Map;
use serde_json::Value;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::apply_patch::apply_patch_payload_command;
use crate::tools::handlers::local_shell_payload_command;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::shell_command_payload_command;
use crate::tools::handlers::shell_function_payload_command;
use crate::tools::handlers::unified_exec::ExecCommandArgs;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::PreToolUsePayload;

/// Projects native tool payloads into the stable input shape exposed to hooks.
///
/// The hook protocol intentionally hides a few native tool-schema differences:
///
/// - Bash-like tools are exposed as `Bash` with `{ "command": <string> }`,
///   even though their native payloads store commands as argv, `command`, or
///   `cmd` depending on the concrete tool.
/// - `apply_patch` exposes its raw patch body through `{ "command": <patch> }`
///   for compatibility with existing hook consumers.
/// - MCP tools already use arbitrary JSON argument objects, so hooks see those
///   arguments directly rather than through a compatibility shape.
pub(crate) fn pre_tool_use_payload(invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
    match invocation.tool_name.name.as_str() {
        "shell" | "container.exec" => {
            shell_function_payload_command(&invocation.payload).map(bash_payload)
        }
        "local_shell" => local_shell_payload_command(&invocation.payload).map(bash_payload),
        "shell_command" => shell_command_payload_command(&invocation.payload).map(bash_payload),
        "exec_command" => exec_command_payload_command(&invocation.payload).map(bash_payload),
        "apply_patch" => {
            apply_patch_payload_command(&invocation.payload).map(|command| PreToolUsePayload {
                tool_name: HookToolName::apply_patch(),
                tool_input: serde_json::json!({ "command": command }),
            })
        }
        _ => mcp_payload(invocation),
    }
}

/// Rebuilds native tool payloads from hook-facing `updatedInput`.
///
/// This is the inverse of [`pre_tool_use_payload`]: Bash-like and
/// `apply_patch` updates come back through the compatibility `{ "command": ... }`
/// shape and must be written into each tool's native schema, while MCP updates
/// replace the raw argument object wholesale because the hook-facing and native
/// representations are the same.
pub(crate) fn apply_updated_input(
    invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    match invocation.tool_name.name.as_str() {
        "shell" => rewrite_shell_function_updated_input(invocation, updated_input, "shell"),
        "container.exec" => {
            rewrite_shell_function_updated_input(invocation, updated_input, "container.exec")
        }
        "local_shell" => rewrite_local_shell_updated_input(invocation, updated_input),
        "shell_command" => rewrite_shell_command_updated_input(invocation, updated_input),
        "exec_command" => rewrite_exec_command_updated_input(invocation, updated_input),
        "apply_patch" => rewrite_apply_patch_updated_input(invocation, updated_input),
        _ => rewrite_mcp_updated_input(invocation, updated_input),
    }
}

fn bash_payload(command: String) -> PreToolUsePayload {
    PreToolUsePayload {
        tool_name: HookToolName::bash(),
        tool_input: serde_json::json!({ "command": command }),
    }
}

fn exec_command_payload_command(payload: &ToolPayload) -> Option<String> {
    let ToolPayload::Function { arguments } = payload else {
        return None;
    };

    parse_arguments::<ExecCommandArgs>(arguments)
        .ok()
        .map(|args| args.cmd)
}

fn mcp_payload(invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
    let ToolPayload::Mcp { raw_arguments, .. } = &invocation.payload else {
        return None;
    };

    Some(PreToolUsePayload {
        tool_name: HookToolName::new(invocation.tool_name.display()),
        tool_input: mcp_hook_tool_input(raw_arguments),
    })
}

fn updated_hook_command(updated_input: &Value) -> Result<&str, FunctionCallError> {
    updated_input
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "hook returned updatedInput without string field `command`".to_string(),
            )
        })
}

fn rewrite_function_arguments(
    arguments: &str,
    tool_name: &str,
    rewrite: impl FnOnce(&mut Map<String, Value>),
) -> Result<String, FunctionCallError> {
    let mut arguments: Value = parse_arguments(arguments)?;
    let Value::Object(arguments) = &mut arguments else {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name} arguments must be an object"
        )));
    };
    rewrite(arguments);
    serde_json::to_string(&arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to serialize rewritten {tool_name} arguments: {err}"
        ))
    })
}

fn rewrite_function_string_argument(
    arguments: &str,
    tool_name: &str,
    field_name: &str,
    value: &str,
) -> Result<String, FunctionCallError> {
    rewrite_function_arguments(arguments, tool_name, |arguments| {
        arguments.insert(field_name.to_string(), Value::String(value.to_string()));
    })
}

/// Rehydrates legacy function-style shell tools from hook-facing command text.
fn rewrite_shell_function_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
    tool_name: &str,
) -> Result<ToolInvocation, FunctionCallError> {
    let ToolPayload::Function { arguments } = invocation.payload else {
        return Err(FunctionCallError::RespondToModel(format!(
            "hook input rewrite received unsupported {tool_name} payload"
        )));
    };
    let command = shlex::split(updated_hook_command(&updated_input)?).ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "hook returned shell input with an invalid command string".to_string(),
        )
    })?;
    invocation.payload = ToolPayload::Function {
        arguments: rewrite_function_arguments(&arguments, tool_name, |arguments| {
            arguments.insert(
                "command".to_string(),
                Value::Array(command.into_iter().map(Value::String).collect()),
            );
        })?,
    };
    Ok(invocation)
}

/// Rehydrates `local_shell` argv from hook-facing command text.
fn rewrite_local_shell_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    let command = updated_hook_command(&updated_input)?;
    invocation.payload = match invocation.payload {
        ToolPayload::LocalShell { mut params } => {
            params.command = shlex::split(command).ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "hook returned shell input with an invalid command string".to_string(),
                )
            })?;
            ToolPayload::LocalShell { params }
        }
        payload => payload,
    };
    Ok(invocation)
}

/// Stores hook-facing command text back into the native `shell_command.command`.
fn rewrite_shell_command_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    let ToolPayload::Function { arguments } = invocation.payload else {
        return Err(FunctionCallError::RespondToModel(
            "hook input rewrite received unsupported shell_command payload".to_string(),
        ));
    };
    invocation.payload = ToolPayload::Function {
        arguments: rewrite_function_string_argument(
            &arguments,
            "shell_command",
            "command",
            updated_hook_command(&updated_input)?,
        )?,
    };
    Ok(invocation)
}

/// Stores hook-facing command text back into the native `exec_command.cmd`.
fn rewrite_exec_command_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    let ToolPayload::Function { arguments } = invocation.payload else {
        return Err(FunctionCallError::RespondToModel(
            "hook input rewrite received unsupported exec_command payload".to_string(),
        ));
    };
    invocation.payload = ToolPayload::Function {
        arguments: rewrite_function_string_argument(
            &arguments,
            "exec_command",
            "cmd",
            updated_hook_command(&updated_input)?,
        )?,
    };
    Ok(invocation)
}

/// Stores hook-facing patch text back into the native apply_patch payload form.
fn rewrite_apply_patch_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    let patch = updated_hook_command(&updated_input)?;
    invocation.payload = match invocation.payload {
        ToolPayload::Function { arguments } => ToolPayload::Function {
            arguments: rewrite_function_string_argument(&arguments, "apply_patch", "input", patch)?,
        },
        ToolPayload::Custom { .. } => ToolPayload::Custom {
            input: patch.to_string(),
        },
        payload => payload,
    };
    Ok(invocation)
}

/// Replaces MCP raw arguments directly because MCP hooks expose that JSON object as-is.
fn rewrite_mcp_updated_input(
    mut invocation: ToolInvocation,
    updated_input: Value,
) -> Result<ToolInvocation, FunctionCallError> {
    invocation.payload = match invocation.payload {
        ToolPayload::Mcp { server, tool, .. } => ToolPayload::Mcp {
            server,
            tool,
            raw_arguments: serde_json::to_string(&updated_input).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to serialize rewritten MCP arguments: {err}"
                ))
            })?,
        },
        payload => {
            return Err(FunctionCallError::RespondToModel(format!(
                "tool {} does not support hook input rewriting for payload {payload:?}",
                invocation.tool_name.display()
            )));
        }
    };
    Ok(invocation)
}

pub(crate) fn mcp_hook_tool_input(raw_arguments: &str) -> Value {
    if raw_arguments.trim().is_empty() {
        return Value::Object(Map::new());
    }

    serde_json::from_str(raw_arguments).unwrap_or_else(|_| Value::String(raw_arguments.to_string()))
}
