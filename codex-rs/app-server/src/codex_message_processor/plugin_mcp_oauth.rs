use std::collections::HashMap;
use std::sync::Arc;

use codex_app_server_protocol::McpServerOauthLoginCompletedNotification;
use codex_app_server_protocol::ServerNotification;
use codex_config::types::McpServerConfig;
use codex_core::config::Config;
use codex_mcp::McpOAuthLoginOutcome;
use codex_mcp::McpRuntimeEnvironment;
use codex_mcp::perform_oauth_login_silent_for_server;

use super::CodexMessageProcessor;

impl CodexMessageProcessor {
    pub(super) async fn start_plugin_mcp_oauth_logins(
        &self,
        config: &Config,
        plugin_mcp_servers: HashMap<String, McpServerConfig>,
    ) {
        for (name, server) in plugin_mcp_servers {
            let environment_manager = self.thread_manager.environment_manager();
            let runtime_environment = match environment_manager.default_environment() {
                Some(environment) => {
                    McpRuntimeEnvironment::new(environment, config.cwd.to_path_buf())
                }
                None => McpRuntimeEnvironment::new(
                    environment_manager.local_environment(),
                    config.cwd.to_path_buf(),
                ),
            };

            let store_mode = config.mcp_oauth_credentials_store_mode;
            let callback_port = config.mcp_oauth_callback_port;
            let callback_url = config.mcp_oauth_callback_url.clone();
            let outgoing = Arc::clone(&self.outgoing);
            let notification_name = name.clone();

            tokio::spawn(async move {
                let final_result = perform_oauth_login_silent_for_server(
                    &name,
                    &server,
                    store_mode,
                    /*explicit_scopes*/ None,
                    callback_port,
                    callback_url.as_deref(),
                    runtime_environment,
                )
                .await;

                let (success, error) = match final_result {
                    Ok(McpOAuthLoginOutcome::Completed) => (true, None),
                    Ok(McpOAuthLoginOutcome::Unsupported) => return,
                    Err(err) => (false, Some(err.to_string())),
                };

                let notification = ServerNotification::McpServerOauthLoginCompleted(
                    McpServerOauthLoginCompletedNotification {
                        name: notification_name,
                        success,
                        error,
                    },
                );
                outgoing.send_server_notification(notification).await;
            });
        }
    }
}
