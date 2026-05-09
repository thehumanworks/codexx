use std::path::PathBuf;

const DEFAULT_REMOTE_CWD: &str = "/workspace";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteSessionRequest {
    pub(crate) provider: RemoteProvider,
    pub(crate) sandbox_id: Option<String>,
    pub(crate) remote_cwd: PathBuf,
    pub(crate) workspace_mode: RemoteWorkspaceMode,
    pub(crate) local_cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteSessionEndpoint {
    pub(crate) websocket_url: String,
    pub(crate) auth_token: String,
    pub(crate) remote_cwd: PathBuf,
    pub(crate) sandbox_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteSandboxSession {
    pub(crate) provider: RemoteProvider,
    pub(crate) sandbox_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteProvider {
    Modal,
}

impl RemoteProvider {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Modal => "modal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteWorkspaceMode {
    UseRemotePath,
    CopyCwd,
    GitClone,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum RemoteSessionParseError {
    #[error("Could not parse /modal arguments: unmatched quote.")]
    UnmatchedQuote,
    #[error("Unknown /modal option `{option}`.")]
    UnknownOption { option: String },
    #[error("Only one Modal sandbox target may be supplied.")]
    DuplicateTarget,
    #[error("`--copy-cwd` and `--git-clone` cannot be used together.")]
    ConflictingWorkspaceModes,
    #[error("Modal workdir must be absolute.")]
    RelativeRemotePath,
    #[error("Modal sandbox target cannot be empty.")]
    EmptyTarget,
}

pub(crate) fn parse_modal_session_request(
    args: &str,
    local_cwd: PathBuf,
) -> Result<RemoteSessionRequest, RemoteSessionParseError> {
    let tokens = shlex::split(args).ok_or(RemoteSessionParseError::UnmatchedQuote)?;

    let mut target: Option<RemoteTarget> = None;
    let mut workspace_mode = RemoteWorkspaceMode::UseRemotePath;
    for token in &tokens {
        match token.as_str() {
            "--copy-cwd" => {
                set_workspace_mode(&mut workspace_mode, RemoteWorkspaceMode::CopyCwd)?;
            }
            "--git-clone" => {
                set_workspace_mode(&mut workspace_mode, RemoteWorkspaceMode::GitClone)?;
            }
            option if option.starts_with('-') => {
                return Err(RemoteSessionParseError::UnknownOption {
                    option: option.to_string(),
                });
            }
            raw_target => {
                if target.is_some() {
                    return Err(RemoteSessionParseError::DuplicateTarget);
                }
                target = Some(parse_remote_target(raw_target)?);
            }
        }
    }

    let target = target.unwrap_or_default();
    Ok(RemoteSessionRequest {
        provider: RemoteProvider::Modal,
        sandbox_id: target.sandbox_id,
        remote_cwd: PathBuf::from(
            target
                .remote_cwd
                .unwrap_or_else(|| DEFAULT_REMOTE_CWD.into()),
        ),
        workspace_mode,
        local_cwd,
    })
}

fn set_workspace_mode(
    current: &mut RemoteWorkspaceMode,
    next: RemoteWorkspaceMode,
) -> Result<(), RemoteSessionParseError> {
    if *current != RemoteWorkspaceMode::UseRemotePath && *current != next {
        return Err(RemoteSessionParseError::ConflictingWorkspaceModes);
    }
    *current = next;
    Ok(())
}

#[derive(Default)]
struct RemoteTarget {
    sandbox_id: Option<String>,
    remote_cwd: Option<String>,
}

fn parse_remote_target(raw: &str) -> Result<RemoteTarget, RemoteSessionParseError> {
    if raw.is_empty() || raw == ":" {
        return Err(RemoteSessionParseError::EmptyTarget);
    }

    if raw.starts_with('/') {
        return Ok(RemoteTarget {
            sandbox_id: None,
            remote_cwd: Some(raw.to_string()),
        });
    }

    let Some((sandbox_id, remote_cwd)) = raw.split_once(':') else {
        return Ok(RemoteTarget {
            sandbox_id: Some(raw.to_string()),
            remote_cwd: None,
        });
    };

    let sandbox_id = (!sandbox_id.is_empty()).then(|| sandbox_id.to_string());
    let remote_cwd = if remote_cwd.is_empty() {
        None
    } else {
        if !remote_cwd.starts_with('/') {
            return Err(RemoteSessionParseError::RelativeRemotePath);
        }
        Some(remote_cwd.to_string())
    };

    Ok(RemoteTarget {
        sandbox_id,
        remote_cwd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn local_cwd() -> PathBuf {
        PathBuf::from("/Users/mish/projects/codex")
    }

    #[test]
    fn parses_empty_modal_args_with_defaults() {
        assert_eq!(
            parse_modal_session_request("", local_cwd()),
            Ok(RemoteSessionRequest {
                provider: RemoteProvider::Modal,
                sandbox_id: None,
                remote_cwd: PathBuf::from("/workspace"),
                workspace_mode: RemoteWorkspaceMode::UseRemotePath,
                local_cwd: local_cwd(),
            })
        );
    }

    #[test]
    fn parses_existing_sandbox_and_remote_path() {
        assert_eq!(
            parse_modal_session_request("sb-123456789:/workspace/codex", local_cwd()),
            Ok(RemoteSessionRequest {
                provider: RemoteProvider::Modal,
                sandbox_id: Some("sb-123456789".to_string()),
                remote_cwd: PathBuf::from("/workspace/codex"),
                workspace_mode: RemoteWorkspaceMode::UseRemotePath,
                local_cwd: local_cwd(),
            })
        );
    }

    #[test]
    fn parses_path_only_target_and_copy_mode() {
        assert_eq!(
            parse_modal_session_request(":/repo --copy-cwd", local_cwd()),
            Ok(RemoteSessionRequest {
                provider: RemoteProvider::Modal,
                sandbox_id: None,
                remote_cwd: PathBuf::from("/repo"),
                workspace_mode: RemoteWorkspaceMode::CopyCwd,
                local_cwd: local_cwd(),
            })
        );
    }

    #[test]
    fn parses_git_clone_mode() {
        assert_eq!(
            parse_modal_session_request("--git-clone", local_cwd()),
            Ok(RemoteSessionRequest {
                provider: RemoteProvider::Modal,
                sandbox_id: None,
                remote_cwd: PathBuf::from("/workspace"),
                workspace_mode: RemoteWorkspaceMode::GitClone,
                local_cwd: local_cwd(),
            })
        );
    }

    #[test]
    fn parses_bare_token_as_sandbox_id() {
        assert_eq!(
            parse_modal_session_request("sb-123456789", local_cwd()),
            Ok(RemoteSessionRequest {
                provider: RemoteProvider::Modal,
                sandbox_id: Some("sb-123456789".to_string()),
                remote_cwd: PathBuf::from("/workspace"),
                workspace_mode: RemoteWorkspaceMode::UseRemotePath,
                local_cwd: local_cwd(),
            })
        );
    }

    #[test]
    fn rejects_conflicting_workspace_modes() {
        assert_eq!(
            parse_modal_session_request("--copy-cwd --git-clone", local_cwd()),
            Err(RemoteSessionParseError::ConflictingWorkspaceModes)
        );
    }

    #[test]
    fn rejects_relative_remote_path() {
        assert_eq!(
            parse_modal_session_request("sb-123:relative/path", local_cwd()),
            Err(RemoteSessionParseError::RelativeRemotePath)
        );
    }
}
