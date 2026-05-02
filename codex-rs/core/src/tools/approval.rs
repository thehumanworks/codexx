//! Canonical approval request routing for approval-requiring actions.
//!
//! Callers describe the action once as an [`ApprovalRequest`]. This module owns
//! the shared approval pipeline for those requests:
//! 1. policy hooks
//! 2. guardian review
//! 3. user prompting, including session-scoped approval caching

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use codex_hooks::PermissionRequestDecision;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::approvals::GuardianCommandSource;
use codex_protocol::approvals::NetworkApprovalContext;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::Future;
use serde::Serialize;

use crate::guardian::GuardianApprovalRequest;
use crate::guardian::GuardianMcpAnnotations;
use crate::guardian::GuardianNetworkAccessTrigger;
use crate::guardian::review_approval_request;
use crate::hook_runtime::run_permission_request_hooks;
use crate::sandboxing::SandboxPermissions;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::hook_names::HookToolName;
use crate::tools::sandboxing::PermissionRequestPayload;

#[derive(Clone, Default, Debug)]
pub(crate) struct ApprovalStore {
    map: HashMap<String, ReviewDecision>,
}

impl ApprovalStore {
    pub(crate) fn get<K>(&self, key: &K) -> Option<ReviewDecision>
    where
        K: Serialize,
    {
        let serialized = serde_json::to_string(key).ok()?;
        self.map.get(&serialized).cloned()
    }

    pub(crate) fn put<K>(&mut self, key: K, value: ReviewDecision)
    where
        K: Serialize,
    {
        if let Ok(serialized) = serde_json::to_string(&key) {
            self.map.insert(serialized, value);
        }
    }
}

async fn with_cached_approval<K, F, Fut>(
    session: &Session,
    tool_name: &str,
    keys: Vec<K>,
    fetch: F,
) -> ReviewDecision
where
    K: Serialize,
    F: FnOnce() -> Fut,
    Fut: Future<Output = ReviewDecision>,
{
    if keys.is_empty() {
        return fetch().await;
    }

    let already_approved = {
        let store = session.services.tool_approvals.lock().await;
        keys.iter()
            .all(|key| matches!(store.get(key), Some(ReviewDecision::ApprovedForSession)))
    };

    if already_approved {
        return ReviewDecision::ApprovedForSession;
    }

    let decision = fetch().await;

    session.services.session_telemetry.counter(
        "codex.approval.requested",
        /*inc*/ 1,
        &[
            ("tool", tool_name),
            ("approved", decision.to_opaque_string()),
        ],
    );

    if matches!(decision, ReviewDecision::ApprovedForSession) {
        let mut store = session.services.tool_approvals.lock().await;
        for key in keys {
            store.put(key, ReviewDecision::ApprovedForSession);
        }
    }

    decision
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
struct ApprovalCacheKey {
    namespace: &'static str,
    value: String,
}

#[derive(Clone, Debug)]
struct ApprovalCacheKeys {
    tool_name: &'static str,
    keys: Vec<ApprovalCacheKey>,
}

#[derive(Debug)]
pub(crate) struct ApprovalOutcome {
    pub(crate) decision: ReviewDecision,
    pub(crate) rejection_message: Option<String>,
    pub(crate) source: ApprovalDecisionSource,
}

#[derive(Debug)]
pub(crate) enum ApprovalDecisionSource {
    PermissionRequestHook,
    Guardian { review_id: String },
    User,
}

impl ApprovalDecisionSource {
    pub(crate) fn guardian_review_id(&self) -> Option<&str> {
        match self {
            ApprovalDecisionSource::Guardian { review_id } => Some(review_id),
            ApprovalDecisionSource::PermissionRequestHook | ApprovalDecisionSource::User => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovalRequest {
    pub(crate) hook_run_id: String,
    pub(crate) user_reason: Option<String>,
    pub(crate) guardian_retry_reason: Option<String>,
    pub(crate) kind: ApprovalRequestKind,
    cache: Option<ApprovalCacheKeys>,
}

impl ApprovalRequest {
    pub(crate) fn new(
        hook_run_id: String,
        user_reason: Option<String>,
        guardian_retry_reason: Option<String>,
        kind: ApprovalRequestKind,
    ) -> Self {
        Self {
            hook_run_id,
            user_reason,
            guardian_retry_reason,
            kind,
            cache: None,
        }
    }

    pub(crate) fn with_session_cache<T>(mut self, tool_name: &'static str, keys: Vec<T>) -> Self
    where
        T: Serialize,
    {
        let keys = keys
            .iter()
            .map(|key| {
                serde_json::to_string(key)
                    .ok()
                    .map(|value| ApprovalCacheKey {
                        namespace: tool_name,
                        value,
                    })
            })
            .collect::<Option<Vec<_>>>();
        self.cache = keys
            .filter(|keys| !keys.is_empty())
            .map(|keys| ApprovalCacheKeys { tool_name, keys });
        self
    }

    pub(crate) fn permission_request_payload(&self) -> PermissionRequestPayload {
        match &self.kind {
            ApprovalRequestKind::Command(request) => PermissionRequestPayload::bash(
                request.hook_command.clone(),
                request.justification.clone(),
            ),
            #[cfg(unix)]
            ApprovalRequestKind::Execve(request) => PermissionRequestPayload::bash(
                codex_shell_command::parse_command::shlex_join(&request.command),
                /*description*/ None,
            ),
            ApprovalRequestKind::Patch(request) => PermissionRequestPayload {
                tool_name: HookToolName::apply_patch(),
                tool_input: serde_json::json!({ "command": request.patch }),
            },
            ApprovalRequestKind::NetworkAccess(request) => PermissionRequestPayload::bash(
                request.hook_command.clone(),
                Some(format!("network-access {}", request.target)),
            ),
            ApprovalRequestKind::McpToolCall(request) => PermissionRequestPayload {
                tool_name: HookToolName::new(request.hook_tool_name.clone()),
                tool_input: request
                    .arguments
                    .clone()
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            },
        }
    }

    pub(crate) fn into_guardian_request(self) -> GuardianApprovalRequest {
        match self.kind {
            ApprovalRequestKind::Command(request) => match request.source {
                GuardianCommandSource::Shell => GuardianApprovalRequest::Shell {
                    id: request.id,
                    command: request.command,
                    cwd: request.cwd,
                    sandbox_permissions: request.sandbox_permissions,
                    additional_permissions: request.additional_permissions,
                    justification: request.justification,
                },
                GuardianCommandSource::UnifiedExec => GuardianApprovalRequest::ExecCommand {
                    id: request.id,
                    command: request.command,
                    cwd: request.cwd,
                    sandbox_permissions: request.sandbox_permissions,
                    additional_permissions: request.additional_permissions,
                    justification: request.justification,
                    tty: request.tty,
                },
            },
            #[cfg(unix)]
            ApprovalRequestKind::Execve(request) => GuardianApprovalRequest::Execve {
                id: request.id,
                source: request.source,
                program: request.program,
                argv: request.argv,
                cwd: request.cwd,
                additional_permissions: request.additional_permissions,
            },
            ApprovalRequestKind::Patch(request) => GuardianApprovalRequest::ApplyPatch {
                id: request.id,
                cwd: request.cwd,
                files: request.files,
                patch: request.patch,
            },
            ApprovalRequestKind::NetworkAccess(request) => GuardianApprovalRequest::NetworkAccess {
                id: request.id,
                turn_id: request.turn_id,
                target: request.target,
                host: request.host,
                protocol: request.protocol,
                port: request.port,
                trigger: request.trigger,
            },
            ApprovalRequestKind::McpToolCall(request) => GuardianApprovalRequest::McpToolCall {
                id: request.id,
                server: request.server,
                tool_name: request.tool_name,
                arguments: request.arguments,
                connector_id: request.connector_id,
                connector_name: request.connector_name,
                connector_description: request.connector_description,
                tool_title: request.tool_title,
                tool_description: request.tool_description,
                annotations: request.annotations,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ApprovalRequestKind {
    Command(CommandApprovalRequest),
    #[cfg(unix)]
    Execve(ExecveApprovalRequest),
    Patch(PatchApprovalRequest),
    NetworkAccess(NetworkAccessApprovalRequest),
    McpToolCall(McpToolCallApprovalRequest),
}

#[derive(Clone, Debug)]
pub(crate) struct CommandApprovalRequest {
    pub(crate) id: String,
    pub(crate) approval_id: Option<String>,
    pub(crate) source: GuardianCommandSource,
    pub(crate) command: Vec<String>,
    pub(crate) hook_command: String,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) sandbox_permissions: SandboxPermissions,
    pub(crate) additional_permissions: Option<AdditionalPermissionProfile>,
    pub(crate) justification: Option<String>,
    pub(crate) network_approval_context: Option<NetworkApprovalContext>,
    pub(crate) proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    pub(crate) available_decisions: Option<Vec<ReviewDecision>>,
    pub(crate) tty: bool,
}

#[cfg(unix)]
#[derive(Clone, Debug)]
pub(crate) struct ExecveApprovalRequest {
    pub(crate) id: String,
    pub(crate) approval_id: String,
    pub(crate) source: GuardianCommandSource,
    pub(crate) program: String,
    pub(crate) argv: Vec<String>,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) additional_permissions: Option<AdditionalPermissionProfile>,
}

#[derive(Clone, Debug)]
pub(crate) struct PatchApprovalRequest {
    pub(crate) id: String,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) files: Vec<AbsolutePathBuf>,
    pub(crate) patch: String,
    pub(crate) changes: HashMap<PathBuf, FileChange>,
    pub(crate) grant_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct NetworkAccessApprovalRequest {
    pub(crate) id: String,
    pub(crate) turn_id: String,
    pub(crate) target: String,
    pub(crate) host: String,
    pub(crate) protocol: NetworkApprovalProtocol,
    pub(crate) port: u16,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) hook_command: String,
    pub(crate) trigger: Option<GuardianNetworkAccessTrigger>,
}

#[derive(Clone, Debug)]
pub(crate) struct McpToolCallApprovalRequest {
    pub(crate) id: String,
    pub(crate) hook_tool_name: String,
    pub(crate) server: String,
    pub(crate) tool_name: String,
    pub(crate) arguments: Option<serde_json::Value>,
    pub(crate) connector_id: Option<String>,
    pub(crate) connector_name: Option<String>,
    pub(crate) connector_description: Option<String>,
    pub(crate) tool_title: Option<String>,
    pub(crate) tool_description: Option<String>,
    pub(crate) annotations: Option<GuardianMcpAnnotations>,
}

async fn dispatch_user_approval(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    request: ApprovalRequest,
) -> ReviewDecision {
    let ApprovalRequest {
        user_reason, kind, ..
    } = request;
    match kind {
        ApprovalRequestKind::Command(request) => {
            session
                .request_command_approval(
                    turn.as_ref(),
                    request.id,
                    request.approval_id,
                    request.command,
                    request.cwd,
                    user_reason,
                    request.network_approval_context,
                    request.proposed_execpolicy_amendment,
                    request.additional_permissions,
                    request.available_decisions,
                )
                .await
        }
        #[cfg(unix)]
        ApprovalRequestKind::Execve(request) => {
            session
                .request_command_approval(
                    turn.as_ref(),
                    request.id,
                    Some(request.approval_id),
                    request.command,
                    request.cwd,
                    user_reason,
                    /*network_approval_context*/ None,
                    /*proposed_execpolicy_amendment*/ None,
                    request.additional_permissions,
                    Some(vec![ReviewDecision::Approved, ReviewDecision::Abort]),
                )
                .await
        }
        ApprovalRequestKind::Patch(request) => {
            let rx_approve = session
                .request_patch_approval(
                    turn.as_ref(),
                    request.id,
                    request.changes,
                    user_reason,
                    request.grant_root,
                )
                .await;
            rx_approve.await.unwrap_or_default()
        }
        ApprovalRequestKind::NetworkAccess(request) => {
            session
                .request_command_approval(
                    turn.as_ref(),
                    request.id,
                    /*approval_id*/ None,
                    vec!["network-access".to_string(), request.target],
                    request.cwd,
                    user_reason,
                    Some(NetworkApprovalContext {
                        host: request.host,
                        protocol: request.protocol,
                    }),
                    /*proposed_execpolicy_amendment*/ None,
                    /*additional_permissions*/ None,
                    /*available_decisions*/ None,
                )
                .await
        }
        ApprovalRequestKind::McpToolCall(_) => {
            unreachable!("MCP approvals use their own user-prompt transport")
        }
    }
}

async fn request_user_approval(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    request: ApprovalRequest,
) -> ReviewDecision {
    if let Some(cache) = request.cache.clone() {
        with_cached_approval(session, cache.tool_name, cache.keys, || {
            dispatch_user_approval(session, turn, request)
        })
        .await
    } else {
        dispatch_user_approval(session, turn, request).await
    }
}

pub(crate) async fn review_before_user_prompt(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    guardian_review_id: Option<String>,
    evaluate_permission_request_hooks: bool,
    request: &ApprovalRequest,
) -> Option<ApprovalOutcome> {
    if evaluate_permission_request_hooks {
        match run_permission_request_hooks(
            session,
            turn,
            &request.hook_run_id,
            request.permission_request_payload(),
        )
        .await
        {
            Some(PermissionRequestDecision::Allow) => {
                return Some(ApprovalOutcome {
                    decision: ReviewDecision::Approved,
                    rejection_message: None,
                    source: ApprovalDecisionSource::PermissionRequestHook,
                });
            }
            Some(PermissionRequestDecision::Deny { message }) => {
                return Some(ApprovalOutcome {
                    decision: ReviewDecision::Denied,
                    rejection_message: Some(message),
                    source: ApprovalDecisionSource::PermissionRequestHook,
                });
            }
            None => {}
        }
    }

    let guardian_retry_reason = request.guardian_retry_reason.clone();
    if let Some(review_id) = guardian_review_id {
        return Some(ApprovalOutcome {
            decision: review_approval_request(
                session,
                turn,
                review_id.clone(),
                request.clone().into_guardian_request(),
                guardian_retry_reason,
            )
            .await,
            rejection_message: None,
            source: ApprovalDecisionSource::Guardian { review_id },
        });
    }

    None
}

pub(crate) async fn request_approval(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    guardian_review_id: Option<String>,
    evaluate_permission_request_hooks: bool,
    request: ApprovalRequest,
) -> ApprovalOutcome {
    if let Some(outcome) = review_before_user_prompt(
        session,
        turn,
        guardian_review_id,
        evaluate_permission_request_hooks,
        &request,
    )
    .await
    {
        return outcome;
    }

    ApprovalOutcome {
        decision: request_user_approval(session, turn, request).await,
        rejection_message: None,
        source: ApprovalDecisionSource::User,
    }
}
