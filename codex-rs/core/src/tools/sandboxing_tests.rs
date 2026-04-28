use super::*;
use crate::sandboxing::SandboxPermissions;
use crate::tools::hook_names::HookToolName;
use codex_protocol::protocol::GranularApprovalConfig;
use codex_protocol::protocol::NetworkAccess;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn bash_permission_request_payload_omits_missing_description() {
    assert_eq!(
        PermissionRequestPayload::bash("echo hi".to_string(), /*description*/ None),
        PermissionRequestPayload {
            tool_name: HookToolName::bash(),
            tool_input: json!({ "command": "echo hi" }),
        }
    );
}

#[test]
fn bash_permission_request_payload_includes_description_when_present() {
    assert_eq!(
        PermissionRequestPayload::bash(
            "echo hi".to_string(),
            Some("network-access example.com".to_string()),
        ),
        PermissionRequestPayload {
            tool_name: HookToolName::bash(),
            tool_input: json!({
                "command": "echo hi",
                "description": "network-access example.com",
            }),
        }
    );
}

#[test]
fn external_sandbox_skips_exec_approval_on_request() {
    let sandbox_policy = SandboxPolicy::ExternalSandbox {
        network_access: NetworkAccess::Restricted,
    };
    assert_eq!(
        default_exec_approval_requirement(
            AskForApproval::OnRequest,
            &FileSystemSandboxPolicy::from(&sandbox_policy),
        ),
        ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
            proposed_execpolicy_amendment: None,
        }
    );
}

#[test]
fn restricted_sandbox_requires_exec_approval_on_request() {
    let sandbox_policy = SandboxPolicy::new_read_only_policy();
    assert_eq!(
        default_exec_approval_requirement(
            AskForApproval::OnRequest,
            &FileSystemSandboxPolicy::from(&sandbox_policy)
        ),
        ExecApprovalRequirement::NeedsApproval {
            reason: None,
            proposed_execpolicy_amendment: None,
        }
    );
}

#[test]
fn default_exec_approval_requirement_rejects_sandbox_prompt_when_granular_disables_it() {
    let policy = AskForApproval::Granular(GranularApprovalConfig {
        sandbox_approval: false,
        rules: true,
        skill_approval: true,
        request_permissions: true,
        mcp_elicitations: true,
    });

    let sandbox_policy = SandboxPolicy::new_read_only_policy();
    let requirement =
        default_exec_approval_requirement(policy, &FileSystemSandboxPolicy::from(&sandbox_policy));

    assert_eq!(
        requirement,
        ExecApprovalRequirement::Forbidden {
            reason: "approval policy disallowed sandbox approval prompt".to_string(),
        }
    );
}

#[test]
fn default_exec_approval_requirement_keeps_prompt_when_granular_allows_sandbox_approval() {
    let policy = AskForApproval::Granular(GranularApprovalConfig {
        sandbox_approval: true,
        rules: false,
        skill_approval: true,
        request_permissions: true,
        mcp_elicitations: false,
    });

    let sandbox_policy = SandboxPolicy::new_read_only_policy();
    let requirement =
        default_exec_approval_requirement(policy, &FileSystemSandboxPolicy::from(&sandbox_policy));

    assert_eq!(
        requirement,
        ExecApprovalRequirement::NeedsApproval {
            reason: None,
            proposed_execpolicy_amendment: None,
        }
    );
}

#[test]
fn additional_permissions_allow_bypass_sandbox_first_attempt_when_execpolicy_skips() {
    assert_eq!(
        sandbox_override_for_first_attempt(
            SandboxPermissions::WithAdditionalPermissions,
            &ExecApprovalRequirement::Skip {
                bypass_sandbox: true,
                proposed_execpolicy_amendment: None,
            },
        ),
        SandboxOverride::BypassSandboxFirstAttempt
    );
}

#[test]
fn guardian_bypasses_sandbox_for_explicit_escalation_on_first_attempt() {
    assert_eq!(
        sandbox_override_for_first_attempt(
            SandboxPermissions::RequireEscalated,
            &ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
        ),
        SandboxOverride::BypassSandboxFirstAttempt
    );
}

fn metadata_redirect_command(target: &str) -> Vec<String> {
    vec![
        "/bin/bash".to_string(),
        "-lc".to_string(),
        format!("printf pwned > {target}"),
    ]
}

fn workspace_write_file_system_sandbox_policy() -> FileSystemSandboxPolicy {
    FileSystemSandboxPolicy::workspace_write(&[], false, false)
}

#[test]
fn protected_metadata_write_preflight_forbids_only_sandboxed_skip() {
    let repo = tempdir().expect("create tempdir");
    std::fs::create_dir(repo.path().join(".git")).expect("create .git");
    let file_system_sandbox_policy = workspace_write_file_system_sandbox_policy();

    let requirement = apply_protected_metadata_write_preflight(
        ExecApprovalRequirement::Skip {
            bypass_sandbox: false,
            proposed_execpolicy_amendment: None,
        },
        SandboxPermissions::UseDefault,
        &metadata_redirect_command(".git/config"),
        repo.path(),
        repo.path(),
        &file_system_sandbox_policy,
    );

    assert_eq!(
        requirement,
        ExecApprovalRequirement::Forbidden {
            reason: "command targets protected workspace metadata path `.git`".to_string()
        }
    );
}

#[test]
fn protected_metadata_write_preflight_preserves_approval_and_bypass_paths() {
    let repo = tempdir().expect("create tempdir");
    std::fs::create_dir(repo.path().join(".git")).expect("create .git");
    let file_system_sandbox_policy = workspace_write_file_system_sandbox_policy();
    let command = metadata_redirect_command(".git/config");

    let needs_approval = ExecApprovalRequirement::NeedsApproval {
        reason: Some("requires approval".to_string()),
        proposed_execpolicy_amendment: None,
    };
    assert_eq!(
        apply_protected_metadata_write_preflight(
            needs_approval.clone(),
            SandboxPermissions::RequireEscalated,
            &command,
            repo.path(),
            repo.path(),
            &file_system_sandbox_policy,
        ),
        needs_approval
    );

    let bypass_sandbox = ExecApprovalRequirement::Skip {
        bypass_sandbox: true,
        proposed_execpolicy_amendment: None,
    };
    assert_eq!(
        apply_protected_metadata_write_preflight(
            bypass_sandbox.clone(),
            SandboxPermissions::UseDefault,
            &command,
            repo.path(),
            repo.path(),
            &file_system_sandbox_policy,
        ),
        bypass_sandbox
    );
}

#[test]
fn protected_metadata_write_preflight_forbids_sandboxed_approval_prompts() {
    let repo = tempdir().expect("create tempdir");
    std::fs::create_dir(repo.path().join(".git")).expect("create .git");
    let file_system_sandbox_policy = workspace_write_file_system_sandbox_policy();

    let requirement = apply_protected_metadata_write_preflight(
        ExecApprovalRequirement::NeedsApproval {
            reason: Some("dangerous command".to_string()),
            proposed_execpolicy_amendment: None,
        },
        SandboxPermissions::UseDefault,
        &metadata_redirect_command(".git/config"),
        repo.path(),
        repo.path(),
        &file_system_sandbox_policy,
    );

    assert_eq!(
        requirement,
        ExecApprovalRequirement::Forbidden {
            reason: "command targets protected workspace metadata path `.git`".to_string()
        }
    );
}
