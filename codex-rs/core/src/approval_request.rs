use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use codex_analytics::GuardianReviewedAction;
use codex_protocol::approvals::ExecApprovalRequestEvent;
use codex_protocol::approvals::GuardianAssessmentAction;
use codex_protocol::approvals::GuardianCommandSource;
use codex_protocol::approvals::NetworkApprovalContext;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::approvals::NetworkPolicyAmendment;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::ApplyPatchApprovalRequestEvent;
use codex_protocol::protocol::ExecPolicyAmendment;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::request_permissions::RequestPermissionProfile;
use codex_protocol::request_permissions::RequestPermissionsEvent;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Serialize;
use serde_json::Value;

use crate::guardian::GUARDIAN_MAX_ACTION_STRING_TOKENS;
use crate::guardian::guardian_truncate_text;
use crate::tools::hook_names::HookToolName;
use crate::tools::sandboxing::PermissionRequestPayload;

/// Canonical description of an approval-worthy action in core.
///
/// This type should describe the action being reviewed exactly once, with
/// guardian review, approval hooks, and user-prompt transports deriving their
/// own projections from it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ApprovalRequest {
    Command(CommandApprovalRequest),
    ApplyPatch(ApplyPatchApprovalRequest),
    McpToolCall(McpToolCallApprovalRequest),
    RequestPermissions(RequestPermissionsApprovalRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CommandApprovalRequest {
    Shell {
        id: String,
        command: Vec<String>,
        hook_command: String,
        cwd: AbsolutePathBuf,
        sandbox_permissions: crate::sandboxing::SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
        justification: Option<String>,
    },
    ExecCommand {
        id: String,
        command: Vec<String>,
        hook_command: String,
        cwd: AbsolutePathBuf,
        sandbox_permissions: crate::sandboxing::SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
        justification: Option<String>,
        tty: bool,
    },
    #[cfg(unix)]
    Execve {
        id: String,
        source: GuardianCommandSource,
        program: String,
        argv: Vec<String>,
        cwd: AbsolutePathBuf,
        additional_permissions: Option<AdditionalPermissionProfile>,
    },
    NetworkAccess {
        id: String,
        turn_id: String,
        target: String,
        hook_command: String,
        cwd: AbsolutePathBuf,
        host: String,
        protocol: NetworkApprovalProtocol,
        port: u16,
        trigger: Option<GuardianNetworkAccessTrigger>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApplyPatchApprovalRequest {
    pub(crate) id: String,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) files: Vec<AbsolutePathBuf>,
    pub(crate) changes: HashMap<PathBuf, FileChange>,
    pub(crate) patch: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct McpToolCallApprovalRequest {
    pub(crate) id: String,
    pub(crate) server: String,
    pub(crate) tool_name: String,
    pub(crate) hook_tool_name: String,
    pub(crate) arguments: Option<Value>,
    pub(crate) connector_id: Option<String>,
    pub(crate) connector_name: Option<String>,
    pub(crate) connector_description: Option<String>,
    pub(crate) tool_title: Option<String>,
    pub(crate) tool_description: Option<String>,
    pub(crate) annotations: Option<GuardianMcpAnnotations>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestPermissionsApprovalRequest {
    pub(crate) id: String,
    pub(crate) turn_id: String,
    pub(crate) reason: Option<String>,
    pub(crate) permissions: RequestPermissionProfile,
    pub(crate) cwd: AbsolutePathBuf,
}

impl ApprovalRequest {
    pub(crate) fn permission_request_payload(&self) -> Option<PermissionRequestPayload> {
        match self {
            Self::Command(request) => Some(request.permission_request_payload()),
            Self::ApplyPatch(request) => Some(request.permission_request_payload()),
            Self::McpToolCall(request) => Some(request.permission_request_payload()),
            Self::RequestPermissions(_) => None,
        }
    }
}

impl CommandApprovalRequest {
    pub(crate) fn permission_request_payload(&self) -> PermissionRequestPayload {
        match self {
            Self::Shell {
                hook_command,
                justification,
                ..
            }
            | Self::ExecCommand {
                hook_command,
                justification,
                ..
            } => PermissionRequestPayload::bash(hook_command.clone(), justification.clone()),
            #[cfg(unix)]
            Self::Execve { program, argv, .. } => {
                let mut command = vec![program.clone()];
                if argv.len() > 1 {
                    command.extend_from_slice(&argv[1..]);
                }
                PermissionRequestPayload::bash(
                    codex_shell_command::parse_command::shlex_join(&command),
                    /*description*/ None,
                )
            }
            Self::NetworkAccess {
                target,
                hook_command,
                ..
            } => PermissionRequestPayload::bash(
                hook_command.clone(),
                Some(format!("network-access {target}")),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_approval_event(
        &self,
        turn_id: String,
        approval_id: Option<String>,
        reason: Option<String>,
        network_approval_context: Option<NetworkApprovalContext>,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        proposed_network_policy_amendments: Option<Vec<NetworkPolicyAmendment>>,
        available_decisions: Option<Vec<ReviewDecision>>,
    ) -> ExecApprovalRequestEvent {
        match self {
            Self::Shell {
                id,
                command,
                cwd,
                additional_permissions,
                ..
            }
            | Self::ExecCommand {
                id,
                command,
                cwd,
                additional_permissions,
                ..
            } => ExecApprovalRequestEvent {
                call_id: id.clone(),
                approval_id,
                turn_id,
                command: command.clone(),
                cwd: cwd.clone(),
                reason,
                network_approval_context,
                proposed_execpolicy_amendment,
                proposed_network_policy_amendments,
                additional_permissions: additional_permissions.clone(),
                available_decisions,
                parsed_cmd: codex_shell_command::parse_command::parse_command(command),
            },
            #[cfg(unix)]
            Self::Execve {
                id,
                argv,
                cwd,
                additional_permissions,
                ..
            } => ExecApprovalRequestEvent {
                call_id: id.clone(),
                approval_id,
                turn_id,
                command: argv.clone(),
                cwd: cwd.clone(),
                reason,
                network_approval_context,
                proposed_execpolicy_amendment,
                proposed_network_policy_amendments,
                additional_permissions: additional_permissions.clone(),
                available_decisions,
                parsed_cmd: codex_shell_command::parse_command::parse_command(argv),
            },
            Self::NetworkAccess {
                id,
                turn_id,
                target,
                cwd,
                host,
                protocol,
                ..
            } => {
                let command = vec!["network-access".to_string(), target.clone()];
                let network_approval_context = Some(NetworkApprovalContext {
                    host: host.clone(),
                    protocol: *protocol,
                });
                let proposed_network_policy_amendments = proposed_network_policy_amendments
                    .or_else(|| {
                        Some(vec![
                            NetworkPolicyAmendment {
                                host: host.clone(),
                                action: codex_protocol::approvals::NetworkPolicyRuleAction::Allow,
                            },
                            NetworkPolicyAmendment {
                                host: host.clone(),
                                action: codex_protocol::approvals::NetworkPolicyRuleAction::Deny,
                            },
                        ])
                    });
                ExecApprovalRequestEvent {
                    call_id: id.clone(),
                    approval_id,
                    turn_id: turn_id.clone(),
                    command: command.clone(),
                    cwd: cwd.clone(),
                    reason,
                    network_approval_context,
                    proposed_execpolicy_amendment: None,
                    proposed_network_policy_amendments,
                    additional_permissions: None,
                    available_decisions,
                    parsed_cmd: codex_shell_command::parse_command::parse_command(&command),
                }
            }
        }
    }
}

impl ApplyPatchApprovalRequest {
    pub(crate) fn permission_request_payload(&self) -> PermissionRequestPayload {
        PermissionRequestPayload {
            tool_name: HookToolName::apply_patch(),
            tool_input: serde_json::json!({ "command": self.patch }),
        }
    }

    pub(crate) fn apply_patch_approval_event(
        &self,
        turn_id: String,
        reason: Option<String>,
        grant_root: Option<PathBuf>,
    ) -> ApplyPatchApprovalRequestEvent {
        ApplyPatchApprovalRequestEvent {
            call_id: self.id.clone(),
            turn_id,
            changes: self.changes.clone(),
            reason,
            grant_root,
        }
    }
}

impl McpToolCallApprovalRequest {
    pub(crate) fn permission_request_payload(&self) -> PermissionRequestPayload {
        PermissionRequestPayload {
            tool_name: HookToolName::new(self.hook_tool_name.clone()),
            tool_input: self
                .arguments
                .clone()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        }
    }
}

impl RequestPermissionsApprovalRequest {
    pub(crate) fn request_permissions_event(&self) -> RequestPermissionsEvent {
        RequestPermissionsEvent {
            call_id: self.id.clone(),
            turn_id: self.turn_id.clone(),
            reason: self.reason.clone(),
            permissions: self.permissions.clone(),
            cwd: Some(self.cwd.clone()),
        }
    }
}

impl From<CommandApprovalRequest> for ApprovalRequest {
    fn from(value: CommandApprovalRequest) -> Self {
        Self::Command(value)
    }
}

impl From<ApplyPatchApprovalRequest> for ApprovalRequest {
    fn from(value: ApplyPatchApprovalRequest) -> Self {
        Self::ApplyPatch(value)
    }
}

impl From<McpToolCallApprovalRequest> for ApprovalRequest {
    fn from(value: McpToolCallApprovalRequest) -> Self {
        Self::McpToolCall(value)
    }
}

impl From<RequestPermissionsApprovalRequest> for ApprovalRequest {
    fn from(value: RequestPermissionsApprovalRequest) -> Self {
        Self::RequestPermissions(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GuardianNetworkAccessTrigger {
    pub(crate) call_id: String,
    pub(crate) tool_name: String,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) sandbox_permissions: crate::sandboxing::SandboxPermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) additional_permissions: Option<AdditionalPermissionProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) justification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tty: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GuardianMcpAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) open_world_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) read_only_hint: Option<bool>,
}

#[derive(Serialize)]
struct CommandApprovalAction<'a> {
    tool: &'a str,
    command: &'a [String],
    cwd: &'a Path,
    sandbox_permissions: crate::sandboxing::SandboxPermissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_permissions: Option<&'a AdditionalPermissionProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    justification: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tty: Option<bool>,
}

#[cfg(unix)]
#[derive(Serialize)]
struct ExecveApprovalAction<'a> {
    tool: &'a str,
    program: &'a str,
    argv: &'a [String],
    cwd: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_permissions: Option<&'a AdditionalPermissionProfile>,
}

#[derive(Serialize)]
struct McpToolCallApprovalAction<'a> {
    tool: &'static str,
    server: &'a str,
    tool_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connector_id: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connector_name: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connector_description: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_title: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_description: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<&'a GuardianMcpAnnotations>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkAccessApprovalAction<'a> {
    tool: &'static str,
    target: &'a str,
    host: &'a str,
    protocol: NetworkApprovalProtocol,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<&'a GuardianNetworkAccessTrigger>,
}

#[derive(Serialize)]
struct RequestPermissionsApprovalAction<'a> {
    tool: &'static str,
    turn_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a String>,
    permissions: &'a RequestPermissionProfile,
}

fn serialize_guardian_action(value: impl Serialize) -> serde_json::Result<Value> {
    serde_json::to_value(value)
}

fn serialize_command_guardian_action(
    tool: &'static str,
    command: &[String],
    cwd: &Path,
    sandbox_permissions: crate::sandboxing::SandboxPermissions,
    additional_permissions: Option<&AdditionalPermissionProfile>,
    justification: Option<&String>,
    tty: Option<bool>,
) -> serde_json::Result<Value> {
    serialize_guardian_action(CommandApprovalAction {
        tool,
        command,
        cwd,
        sandbox_permissions,
        additional_permissions,
        justification,
        tty,
    })
}

fn command_assessment_action(
    source: GuardianCommandSource,
    command: &[String],
    cwd: &AbsolutePathBuf,
) -> GuardianAssessmentAction {
    GuardianAssessmentAction::Command {
        source,
        command: codex_shell_command::parse_command::shlex_join(command),
        cwd: cwd.clone(),
    }
}

#[cfg(unix)]
fn guardian_command_source_tool_name(source: GuardianCommandSource) -> &'static str {
    match source {
        GuardianCommandSource::Shell => "shell",
        GuardianCommandSource::UnifiedExec => "exec_command",
    }
}

fn truncate_guardian_action_value(value: Value) -> (Value, bool) {
    match value {
        Value::String(text) => {
            let (text, truncated) =
                guardian_truncate_text(&text, GUARDIAN_MAX_ACTION_STRING_TOKENS);
            (Value::String(text), truncated)
        }
        Value::Array(values) => {
            let mut truncated = false;
            let values = values
                .into_iter()
                .map(|value| {
                    let (value, value_truncated) = truncate_guardian_action_value(value);
                    truncated |= value_truncated;
                    value
                })
                .collect::<Vec<_>>();
            (Value::Array(values), truncated)
        }
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut truncated = false;
            let values = entries
                .into_iter()
                .map(|(key, value)| {
                    let (value, value_truncated) = truncate_guardian_action_value(value);
                    truncated |= value_truncated;
                    (key, value)
                })
                .collect();
            (Value::Object(values), truncated)
        }
        other => (other, false),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormattedGuardianAction {
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

pub(crate) fn guardian_approval_request_to_json(
    action: &ApprovalRequest,
) -> serde_json::Result<Value> {
    match action {
        ApprovalRequest::Command(CommandApprovalRequest::Shell {
            id: _,
            command,
            cwd,
            sandbox_permissions,
            additional_permissions,
            justification,
            ..
        }) => serialize_command_guardian_action(
            "shell",
            command,
            cwd,
            *sandbox_permissions,
            additional_permissions.as_ref(),
            justification.as_ref(),
            /*tty*/ None,
        ),
        ApprovalRequest::Command(CommandApprovalRequest::ExecCommand {
            id: _,
            command,
            cwd,
            sandbox_permissions,
            additional_permissions,
            justification,
            tty,
            ..
        }) => serialize_command_guardian_action(
            "exec_command",
            command,
            cwd,
            *sandbox_permissions,
            additional_permissions.as_ref(),
            justification.as_ref(),
            Some(*tty),
        ),
        #[cfg(unix)]
        ApprovalRequest::Command(CommandApprovalRequest::Execve {
            id: _,
            source,
            program,
            argv,
            cwd,
            additional_permissions,
        }) => serialize_guardian_action(ExecveApprovalAction {
            tool: guardian_command_source_tool_name(*source),
            program,
            argv,
            cwd,
            additional_permissions: additional_permissions.as_ref(),
        }),
        ApprovalRequest::ApplyPatch(ApplyPatchApprovalRequest {
            id: _,
            cwd,
            files,
            changes: _,
            patch,
        }) => Ok(serde_json::json!({
            "tool": "apply_patch",
            "cwd": cwd,
            "files": files,
            "patch": patch,
        })),
        ApprovalRequest::Command(CommandApprovalRequest::NetworkAccess {
            id: _,
            turn_id: _,
            target,
            host,
            protocol,
            port,
            trigger,
            ..
        }) => serialize_guardian_action(NetworkAccessApprovalAction {
            tool: "network_access",
            target,
            host,
            protocol: *protocol,
            port: *port,
            trigger: trigger.as_ref(),
        }),
        ApprovalRequest::McpToolCall(McpToolCallApprovalRequest {
            id: _,
            server,
            tool_name,
            arguments,
            connector_id,
            connector_name,
            connector_description,
            tool_title,
            tool_description,
            annotations,
            ..
        }) => serialize_guardian_action(McpToolCallApprovalAction {
            tool: "mcp_tool_call",
            server,
            tool_name,
            arguments: arguments.as_ref(),
            connector_id: connector_id.as_ref(),
            connector_name: connector_name.as_ref(),
            connector_description: connector_description.as_ref(),
            tool_title: tool_title.as_ref(),
            tool_description: tool_description.as_ref(),
            annotations: annotations.as_ref(),
        }),
        ApprovalRequest::RequestPermissions(RequestPermissionsApprovalRequest {
            id: _,
            turn_id,
            reason,
            permissions,
            ..
        }) => serialize_guardian_action(RequestPermissionsApprovalAction {
            tool: "request_permissions",
            turn_id,
            reason: reason.as_ref(),
            permissions,
        }),
    }
}

pub(crate) fn guardian_assessment_action(action: &ApprovalRequest) -> GuardianAssessmentAction {
    match action {
        ApprovalRequest::Command(CommandApprovalRequest::Shell { command, cwd, .. }) => {
            command_assessment_action(GuardianCommandSource::Shell, command, cwd)
        }
        ApprovalRequest::Command(CommandApprovalRequest::ExecCommand { command, cwd, .. }) => {
            command_assessment_action(GuardianCommandSource::UnifiedExec, command, cwd)
        }
        #[cfg(unix)]
        ApprovalRequest::Command(CommandApprovalRequest::Execve {
            source,
            program,
            argv,
            cwd,
            ..
        }) => GuardianAssessmentAction::Execve {
            source: *source,
            program: program.clone(),
            argv: argv.clone(),
            cwd: cwd.clone(),
        },
        ApprovalRequest::ApplyPatch(ApplyPatchApprovalRequest { cwd, files, .. }) => {
            GuardianAssessmentAction::ApplyPatch {
                cwd: cwd.clone(),
                files: files.clone(),
            }
        }
        ApprovalRequest::Command(CommandApprovalRequest::NetworkAccess {
            id: _id,
            turn_id: _turn_id,
            target,
            host,
            protocol,
            port,
            trigger: _trigger,
            ..
        }) => GuardianAssessmentAction::NetworkAccess {
            target: target.clone(),
            host: host.clone(),
            protocol: *protocol,
            port: *port,
        },
        ApprovalRequest::McpToolCall(McpToolCallApprovalRequest {
            server,
            tool_name,
            connector_id,
            connector_name,
            tool_title,
            ..
        }) => GuardianAssessmentAction::McpToolCall {
            server: server.clone(),
            tool_name: tool_name.clone(),
            connector_id: connector_id.clone(),
            connector_name: connector_name.clone(),
            tool_title: tool_title.clone(),
        },
        ApprovalRequest::RequestPermissions(RequestPermissionsApprovalRequest {
            reason,
            permissions,
            ..
        }) => GuardianAssessmentAction::RequestPermissions {
            reason: reason.clone(),
            permissions: permissions.clone(),
        },
    }
}

pub(crate) fn guardian_reviewed_action(request: &ApprovalRequest) -> GuardianReviewedAction {
    match request {
        ApprovalRequest::Command(CommandApprovalRequest::Shell {
            sandbox_permissions,
            additional_permissions,
            ..
        }) => GuardianReviewedAction::Shell {
            sandbox_permissions: *sandbox_permissions,
            additional_permissions: additional_permissions.clone(),
        },
        ApprovalRequest::Command(CommandApprovalRequest::ExecCommand {
            sandbox_permissions,
            additional_permissions,
            tty,
            ..
        }) => GuardianReviewedAction::UnifiedExec {
            sandbox_permissions: *sandbox_permissions,
            additional_permissions: additional_permissions.clone(),
            tty: *tty,
        },
        #[cfg(unix)]
        ApprovalRequest::Command(CommandApprovalRequest::Execve {
            source,
            program,
            additional_permissions,
            ..
        }) => GuardianReviewedAction::Execve {
            source: *source,
            program: program.clone(),
            additional_permissions: additional_permissions.clone(),
        },
        ApprovalRequest::ApplyPatch(..) => GuardianReviewedAction::ApplyPatch {},
        ApprovalRequest::Command(CommandApprovalRequest::NetworkAccess {
            protocol, port, ..
        }) => GuardianReviewedAction::NetworkAccess {
            protocol: *protocol,
            port: *port,
        },
        ApprovalRequest::McpToolCall(McpToolCallApprovalRequest {
            server,
            tool_name,
            connector_id,
            connector_name,
            tool_title,
            ..
        }) => GuardianReviewedAction::McpToolCall {
            server: server.clone(),
            tool_name: tool_name.clone(),
            connector_id: connector_id.clone(),
            connector_name: connector_name.clone(),
            tool_title: tool_title.clone(),
        },
        ApprovalRequest::RequestPermissions(..) => GuardianReviewedAction::RequestPermissions {},
    }
}

pub(crate) fn guardian_request_target_item_id(request: &ApprovalRequest) -> Option<&str> {
    match request {
        ApprovalRequest::Command(CommandApprovalRequest::Shell { id, .. })
        | ApprovalRequest::Command(CommandApprovalRequest::ExecCommand { id, .. })
        | ApprovalRequest::ApplyPatch(ApplyPatchApprovalRequest { id, .. })
        | ApprovalRequest::McpToolCall(McpToolCallApprovalRequest { id, .. })
        | ApprovalRequest::RequestPermissions(RequestPermissionsApprovalRequest { id, .. }) => {
            Some(id)
        }
        ApprovalRequest::Command(CommandApprovalRequest::NetworkAccess { .. }) => None,
        #[cfg(unix)]
        ApprovalRequest::Command(CommandApprovalRequest::Execve { id, .. }) => Some(id),
    }
}

pub(crate) fn guardian_request_turn_id<'a>(
    request: &'a ApprovalRequest,
    default_turn_id: &'a str,
) -> &'a str {
    match request {
        ApprovalRequest::Command(CommandApprovalRequest::NetworkAccess { turn_id, .. })
        | ApprovalRequest::RequestPermissions(RequestPermissionsApprovalRequest {
            turn_id, ..
        }) => turn_id,
        ApprovalRequest::Command(CommandApprovalRequest::Shell { .. })
        | ApprovalRequest::Command(CommandApprovalRequest::ExecCommand { .. })
        | ApprovalRequest::ApplyPatch(..)
        | ApprovalRequest::McpToolCall(..) => default_turn_id,
        #[cfg(unix)]
        ApprovalRequest::Command(CommandApprovalRequest::Execve { .. }) => default_turn_id,
    }
}

pub(crate) fn format_guardian_action_pretty(
    action: &ApprovalRequest,
) -> serde_json::Result<FormattedGuardianAction> {
    let value = guardian_approval_request_to_json(action)?;
    let (value, truncated) = truncate_guardian_action_value(value);
    Ok(FormattedGuardianAction {
        text: serde_json::to_string_pretty(&value)?,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::FileChange;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    #[test]
    fn exec_approval_event_is_projected_from_shell_request() {
        let request = CommandApprovalRequest::Shell {
            id: "call-1".to_string(),
            command: vec!["echo".to_string(), "hi".to_string()],
            hook_command: "echo hi".to_string(),
            cwd: test_path_buf("/tmp").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("because".to_string()),
        };

        let event = request.exec_approval_event(
            "turn-1".to_string(),
            Some("approval-1".to_string()),
            Some("retry".to_string()),
            /*network_approval_context*/ None,
            /*proposed_execpolicy_amendment*/ None,
            /*proposed_network_policy_amendments*/ None,
            Some(vec![ReviewDecision::Approved, ReviewDecision::Abort]),
        );

        assert_eq!(event.call_id, "call-1");
        assert_eq!(event.approval_id.as_deref(), Some("approval-1"));
        assert_eq!(event.turn_id, "turn-1");
        assert_eq!(event.command, vec!["echo".to_string(), "hi".to_string()]);
        assert_eq!(event.reason.as_deref(), Some("retry"));
        assert_eq!(
            event.available_decisions,
            Some(vec![ReviewDecision::Approved, ReviewDecision::Abort])
        );
    }

    #[test]
    fn apply_patch_approval_event_is_projected_from_request() {
        let path = test_path_buf("/tmp/file.txt");
        let abs_path = path.abs();
        let request = ApplyPatchApprovalRequest {
            id: "call-1".to_string(),
            cwd: test_path_buf("/tmp").abs(),
            files: vec![abs_path],
            changes: HashMap::from([(
                path.clone(),
                FileChange::Add {
                    content: "hello".to_string(),
                },
            )]),
            patch: "*** Begin Patch".to_string(),
        };

        let event = request.apply_patch_approval_event(
            "turn-1".to_string(),
            Some("needs write".to_string()),
            /*grant_root*/ None,
        );

        assert_eq!(event.call_id, "call-1");
        assert_eq!(event.turn_id, "turn-1");
        assert_eq!(event.reason.as_deref(), Some("needs write"));
        assert_eq!(
            event.changes,
            HashMap::from([(
                path,
                FileChange::Add {
                    content: "hello".to_string(),
                },
            )])
        );
    }

    #[test]
    fn request_permissions_event_is_projected_from_request() {
        let request = RequestPermissionsApprovalRequest {
            id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            reason: Some("need outbound network".to_string()),
            permissions: RequestPermissionProfile {
                network: Some(codex_protocol::models::NetworkPermissions {
                    enabled: Some(true),
                }),
                file_system: None,
            },
            cwd: test_path_buf("/tmp").abs(),
        };

        let event = request.request_permissions_event();

        assert_eq!(event.call_id, "call-1");
        assert_eq!(event.turn_id, "turn-1");
        assert_eq!(event.reason.as_deref(), Some("need outbound network"));
        assert_eq!(
            event.permissions,
            RequestPermissionProfile {
                network: Some(codex_protocol::models::NetworkPermissions {
                    enabled: Some(true),
                }),
                file_system: None,
            }
        );
        assert_eq!(event.cwd, Some(test_path_buf("/tmp").abs()));
    }

    #[test]
    fn network_exec_approval_event_is_projected_from_request() {
        let request = CommandApprovalRequest::NetworkAccess {
            id: "network-1".to_string(),
            turn_id: "turn-1".to_string(),
            target: "https://example.com:443".to_string(),
            hook_command: "curl https://example.com".to_string(),
            cwd: test_path_buf("/tmp").abs(),
            host: "example.com".to_string(),
            protocol: NetworkApprovalProtocol::Https,
            port: 443,
            trigger: None,
        };

        let event = request.exec_approval_event(
            "ignored-turn".to_string(),
            /*approval_id*/ None,
            Some("need network".to_string()),
            /*network_approval_context*/ None,
            /*proposed_execpolicy_amendment*/ None,
            /*proposed_network_policy_amendments*/ None,
            /*available_decisions*/ None,
        );

        assert_eq!(event.call_id, "network-1");
        assert_eq!(event.turn_id, "turn-1");
        assert_eq!(
            event.command,
            vec![
                "network-access".to_string(),
                "https://example.com:443".to_string()
            ]
        );
        assert_eq!(event.cwd, test_path_buf("/tmp").abs());
        assert_eq!(event.reason.as_deref(), Some("need network"));
        assert_eq!(
            event.network_approval_context,
            Some(NetworkApprovalContext {
                host: "example.com".to_string(),
                protocol: NetworkApprovalProtocol::Https,
            })
        );
        assert_eq!(
            event.proposed_network_policy_amendments,
            Some(vec![
                NetworkPolicyAmendment {
                    host: "example.com".to_string(),
                    action: codex_protocol::approvals::NetworkPolicyRuleAction::Allow,
                },
                NetworkPolicyAmendment {
                    host: "example.com".to_string(),
                    action: codex_protocol::approvals::NetworkPolicyRuleAction::Deny,
                },
            ])
        );
    }
}
