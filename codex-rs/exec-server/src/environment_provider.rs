use std::collections::HashMap;

use async_trait::async_trait;

use crate::Environment;
use crate::ExecServerError;
use crate::ExecServerRuntimePaths;
use crate::environment::CODEX_EXEC_SERVER_ENVIRONMENT_ID_ENV_VAR;
use crate::environment::CODEX_EXEC_SERVER_URL_ENV_VAR;
use crate::environment::LOCAL_ENVIRONMENT_ID;
use crate::environment::REMOTE_ENVIRONMENT_ID;

/// Lists the concrete environments available to Codex.
///
/// Implementations own a startup snapshot containing both the available
/// environment list and default environment selection. Providers that want the
/// local environment to be addressable by id should include it explicitly in
/// the returned map.
#[async_trait]
pub trait EnvironmentProvider: Send + Sync {
    /// Returns the provider-owned environment startup snapshot.
    async fn snapshot(
        &self,
        local_runtime_paths: &ExecServerRuntimePaths,
    ) -> Result<EnvironmentProviderSnapshot, ExecServerError>;
}

#[derive(Clone, Debug)]
pub struct EnvironmentProviderSnapshot {
    pub environments: HashMap<String, Environment>,
    pub default: EnvironmentDefault,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentDefault {
    Disabled,
    EnvironmentId(String),
}

/// Default provider backed by `CODEX_EXEC_SERVER_URL`.
#[derive(Clone, Debug)]
pub struct DefaultEnvironmentProvider {
    exec_server_url: Option<String>,
    remote_environment_id: String,
}

impl DefaultEnvironmentProvider {
    /// Builds a provider from an already-read raw `CODEX_EXEC_SERVER_URL` value.
    pub fn new(exec_server_url: Option<String>) -> Self {
        Self {
            exec_server_url,
            remote_environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        }
    }

    /// Builds a provider by reading `CODEX_EXEC_SERVER_URL` and its companion settings.
    pub fn from_env() -> Self {
        Self {
            exec_server_url: std::env::var(CODEX_EXEC_SERVER_URL_ENV_VAR).ok(),
            remote_environment_id: normalize_remote_environment_id(
                std::env::var(CODEX_EXEC_SERVER_ENVIRONMENT_ID_ENV_VAR).ok(),
            ),
        }
    }

    #[cfg(test)]
    fn with_remote_environment_id(mut self, remote_environment_id: impl Into<String>) -> Self {
        self.remote_environment_id =
            normalize_remote_environment_id(Some(remote_environment_id.into()));
        self
    }

    pub(crate) fn snapshot_inner(
        &self,
        local_runtime_paths: &ExecServerRuntimePaths,
    ) -> EnvironmentProviderSnapshot {
        let mut environments = HashMap::from([(
            LOCAL_ENVIRONMENT_ID.to_string(),
            Environment::local(local_runtime_paths.clone()),
        )]);
        let (exec_server_url, disabled) = normalize_exec_server_url(self.exec_server_url.clone());
        let has_remote_environment = exec_server_url.is_some();

        if let Some(exec_server_url) = exec_server_url {
            environments.insert(
                self.remote_environment_id.clone(),
                Environment::remote_inner(exec_server_url, Some(local_runtime_paths.clone())),
            );
        }

        let default = if disabled {
            EnvironmentDefault::Disabled
        } else if has_remote_environment {
            EnvironmentDefault::EnvironmentId(self.remote_environment_id.clone())
        } else {
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        };

        EnvironmentProviderSnapshot {
            environments,
            default,
        }
    }
}

#[async_trait]
impl EnvironmentProvider for DefaultEnvironmentProvider {
    async fn snapshot(
        &self,
        local_runtime_paths: &ExecServerRuntimePaths,
    ) -> Result<EnvironmentProviderSnapshot, ExecServerError> {
        Ok(self.snapshot_inner(local_runtime_paths))
    }
}

pub(crate) fn normalize_exec_server_url(exec_server_url: Option<String>) -> (Option<String>, bool) {
    match exec_server_url.as_deref().map(str::trim) {
        None | Some("") => (None, false),
        Some(url) if url.eq_ignore_ascii_case("none") => (None, true),
        Some(url) => (Some(url.to_string()), false),
    }
}

fn normalize_remote_environment_id(remote_environment_id: Option<String>) -> String {
    match remote_environment_id.as_deref().map(str::trim) {
        Some("") | None => REMOTE_ENVIRONMENT_ID.to_string(),
        Some(remote_environment_id) => remote_environment_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::ExecServerRuntimePaths;

    fn test_runtime_paths() -> ExecServerRuntimePaths {
        ExecServerRuntimePaths::new(
            std::env::current_exe().expect("current exe"),
            /*codex_linux_sandbox_exe*/ None,
        )
        .expect("runtime paths")
    }

    #[tokio::test]
    async fn default_provider_returns_local_environment_when_url_is_missing() {
        let provider = DefaultEnvironmentProvider::new(/*exec_server_url*/ None);
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");
        let environments = snapshot.environments;

        assert!(!environments[LOCAL_ENVIRONMENT_ID].is_remote());
        assert_eq!(
            environments[LOCAL_ENVIRONMENT_ID].local_runtime_paths(),
            Some(&runtime_paths)
        );
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(
            snapshot.default,
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_returns_local_environment_when_url_is_empty() {
        let provider = DefaultEnvironmentProvider::new(Some(String::new()));
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");
        let environments = snapshot.environments;

        assert!(!environments[LOCAL_ENVIRONMENT_ID].is_remote());
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(
            snapshot.default,
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_returns_local_environment_for_none_value() {
        let provider = DefaultEnvironmentProvider::new(Some("none".to_string()));
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");
        let environments = snapshot.environments;

        assert!(!environments[LOCAL_ENVIRONMENT_ID].is_remote());
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(snapshot.default, EnvironmentDefault::Disabled);
    }

    #[tokio::test]
    async fn default_provider_adds_remote_environment_for_websocket_url() {
        let provider = DefaultEnvironmentProvider::new(Some("ws://127.0.0.1:8765".to_string()));
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");
        let environments = snapshot.environments;

        assert!(!environments[LOCAL_ENVIRONMENT_ID].is_remote());
        let remote_environment = &environments[REMOTE_ENVIRONMENT_ID];
        assert!(remote_environment.is_remote());
        assert_eq!(
            remote_environment.exec_server_url(),
            Some("ws://127.0.0.1:8765")
        );
        assert_eq!(
            snapshot.default,
            EnvironmentDefault::EnvironmentId(REMOTE_ENVIRONMENT_ID.to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_normalizes_exec_server_url() {
        let provider = DefaultEnvironmentProvider::new(Some(" ws://127.0.0.1:8765 ".to_string()));
        let runtime_paths = test_runtime_paths();
        let environments = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");

        assert_eq!(
            environments.environments[REMOTE_ENVIRONMENT_ID].exec_server_url(),
            Some("ws://127.0.0.1:8765")
        );
    }

    #[tokio::test]
    async fn default_provider_uses_custom_remote_environment_id() {
        let provider = DefaultEnvironmentProvider::new(Some("ws://127.0.0.1:8765".to_string()))
            .with_remote_environment_id("devbox");
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");

        assert!(!snapshot.environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert!(snapshot.environments["devbox"].is_remote());
        assert_eq!(
            snapshot.default,
            EnvironmentDefault::EnvironmentId("devbox".to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_custom_remote_environment_id_can_replace_local() {
        let provider = DefaultEnvironmentProvider::new(Some("ws://127.0.0.1:8765".to_string()))
            .with_remote_environment_id(LOCAL_ENVIRONMENT_ID);
        let runtime_paths = test_runtime_paths();
        let snapshot = provider
            .snapshot(&runtime_paths)
            .await
            .expect("environments");

        assert!(snapshot.environments[LOCAL_ENVIRONMENT_ID].is_remote());
        assert_eq!(
            snapshot.default,
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        );
    }

    #[test]
    fn remote_environment_id_normalization_uses_default_for_empty_values() {
        assert_eq!(
            normalize_remote_environment_id(Some("  ".to_string())),
            REMOTE_ENVIRONMENT_ID
        );
        assert_eq!(
            normalize_remote_environment_id(/*remote_environment_id*/ None),
            REMOTE_ENVIRONMENT_ID
        );
    }
}
