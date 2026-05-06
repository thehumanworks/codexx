use std::collections::HashSet;

use codex_app_server_protocol::AppInfo;
use codex_config::types::ToolSuggestDisabledTool;
use codex_core::config::Config;
use codex_core::config::edit::ConfigEdit;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::connectors;
use codex_core::extensibility::FunctionCallError;
use codex_core::extensibility::FunctionToolOutput;
use codex_core::extensibility::ToolHandler as ExtensionToolHandler;
use codex_core::extensibility::ToolInvocation;
use codex_core::extensibility::ToolProvider;
use codex_core_plugins::PluginsManager;
use codex_login::CodexAuth;
use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_rmcp_client::ElicitationAction;
use codex_rmcp_client::ElicitationResponse;
use codex_tools::DiscoverableTool;
use codex_tools::DiscoverableToolAction;
use codex_tools::DiscoverableToolType;
use codex_tools::REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE;
use codex_tools::REQUEST_PLUGIN_INSTALL_PERSIST_KEY;
use codex_tools::REQUEST_PLUGIN_INSTALL_TOOL_NAME;
use codex_tools::RequestPluginInstallArgs;
use codex_tools::RequestPluginInstallResult;
use codex_tools::ToolName;
use codex_tools::all_requested_connectors_picked_up;
use codex_tools::build_request_plugin_install_elicitation_request;
use codex_tools::filter_request_plugin_install_discoverable_tools_for_client;
use codex_tools::verified_connector_install_completed;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value;
use tracing::warn;

pub(crate) struct PluginInstallSuggestToolProvider;

impl ToolProvider for PluginInstallSuggestToolProvider {
    fn handlers(&self) -> Vec<ExtensionToolHandler> {
        vec![ExtensionToolHandler::function(
            ToolName::plain(REQUEST_PLUGIN_INSTALL_TOOL_NAME),
            handle_request_plugin_install,
        )]
    }
}

async fn handle_request_plugin_install(
    invocation: ToolInvocation,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let arguments = invocation.function_arguments(REQUEST_PLUGIN_INSTALL_TOOL_NAME)?;
    let args: RequestPluginInstallArgs = serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })?;
    let suggest_reason = args.suggest_reason.trim();
    if suggest_reason.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "suggest_reason must not be empty".to_string(),
        ));
    }
    if args.action_type != DiscoverableToolAction::Install {
        return Err(FunctionCallError::RespondToModel(
            "plugin install requests currently support only action_type=\"install\"".to_string(),
        ));
    }
    if args.tool_type == DiscoverableToolType::Plugin
        && invocation.app_server_client_name() == Some("codex-tui")
    {
        return Err(FunctionCallError::RespondToModel(
            "plugin install requests are not available in codex-tui yet".to_string(),
        ));
    }

    let auth = invocation.auth().await;
    let mcp_tools = invocation.list_mcp_tools().await;
    let accessible_connectors = connectors::with_app_enabled_state(
        connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
        invocation.config(),
    );
    let discoverable_tools = connectors::list_tool_suggest_discoverable_tools_with_auth(
        invocation.config(),
        auth.as_ref(),
        &accessible_connectors,
    )
    .await
    .map(|discoverable_tools| {
        filter_request_plugin_install_discoverable_tools_for_client(
            discoverable_tools,
            invocation.app_server_client_name(),
        )
    })
    .map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "plugin install requests are unavailable right now: {err}"
        ))
    })?;

    let tool = discoverable_tools
        .into_iter()
        .find(|tool| tool.tool_type() == args.tool_type && tool.id() == args.tool_id)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                    "tool_id must match one of the discoverable tools exposed by {REQUEST_PLUGIN_INSTALL_TOOL_NAME}"
                ))
        })?;

    let request_id = format!("request_plugin_install_{}", invocation.call_id());
    let params = build_request_plugin_install_elicitation_request(
        CODEX_APPS_MCP_SERVER_NAME,
        invocation.conversation_id_string(),
        invocation.turn_id(),
        &args,
        suggest_reason,
        &tool,
    );
    let response = invocation
        .request_mcp_server_elicitation(request_id, params)
        .await;
    if let Some(response) = response.as_ref() {
        maybe_persist_disabled_install_request(&invocation, &tool, response).await;
    }
    let user_confirmed = response
        .as_ref()
        .is_some_and(|response| response.action == ElicitationAction::Accept);

    let completed = if user_confirmed {
        verify_request_plugin_install_completed(&invocation, &tool, auth.as_ref()).await
    } else {
        false
    };

    if completed && let DiscoverableTool::Connector(connector) = &tool {
        invocation
            .merge_connector_selection(HashSet::from([connector.id.clone()]))
            .await;
    }

    let content = serde_json::to_string(&RequestPluginInstallResult {
        completed,
        user_confirmed,
        tool_type: args.tool_type,
        action_type: args.action_type,
        tool_id: tool.id().to_string(),
        tool_name: tool.name().to_string(),
        suggest_reason: suggest_reason.to_string(),
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!(
            "failed to serialize {REQUEST_PLUGIN_INSTALL_TOOL_NAME} response: {err}"
        ))
    })?;

    Ok(FunctionToolOutput::from_text(content, Some(true)))
}

async fn maybe_persist_disabled_install_request(
    invocation: &ToolInvocation,
    tool: &DiscoverableTool,
    response: &ElicitationResponse,
) {
    if !request_plugin_install_response_requests_persistent_disable(response) {
        return;
    }

    let codex_home = invocation.codex_home().clone();
    if let Err(err) = persist_disabled_install_request(&codex_home, tool).await {
        warn!(
            error = %err,
            tool_id = tool.id(),
            "failed to persist disabled tool suggestion"
        );
        return;
    }

    invocation.reload_user_config_layer().await;
}

fn request_plugin_install_response_requests_persistent_disable(
    response: &ElicitationResponse,
) -> bool {
    if response.action != ElicitationAction::Decline {
        return false;
    }

    response
        .meta
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|meta| meta.get(REQUEST_PLUGIN_INSTALL_PERSIST_KEY))
        .and_then(Value::as_str)
        == Some(REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE)
}

async fn persist_disabled_install_request(
    codex_home: &AbsolutePathBuf,
    tool: &DiscoverableTool,
) -> anyhow::Result<()> {
    ConfigEditsBuilder::new(codex_home)
        .with_edits([ConfigEdit::AddToolSuggestDisabledTool(
            disabled_install_request(tool),
        )])
        .apply()
        .await
}

fn disabled_install_request(tool: &DiscoverableTool) -> ToolSuggestDisabledTool {
    match tool {
        DiscoverableTool::Connector(connector) => {
            ToolSuggestDisabledTool::connector(connector.id.as_str())
        }
        DiscoverableTool::Plugin(plugin) => ToolSuggestDisabledTool::plugin(plugin.id.as_str()),
    }
}

async fn verify_request_plugin_install_completed(
    invocation: &ToolInvocation,
    tool: &DiscoverableTool,
    auth: Option<&CodexAuth>,
) -> bool {
    match tool {
        DiscoverableTool::Connector(connector) => refresh_missing_requested_connectors(
            invocation,
            auth,
            std::slice::from_ref(&connector.id),
            connector.id.as_str(),
        )
        .await
        .is_some_and(|accessible_connectors| {
            verified_connector_install_completed(connector.id.as_str(), &accessible_connectors)
        }),
        DiscoverableTool::Plugin(plugin) => {
            invocation.reload_user_config_layer().await;
            let completed = verified_plugin_install_completed(
                plugin.id.as_str(),
                invocation.config(),
                invocation.plugins_manager(),
            );
            let _ = refresh_missing_requested_connectors(
                invocation,
                auth,
                &plugin.app_connector_ids,
                plugin.id.as_str(),
            )
            .await;
            completed
        }
    }
}

async fn refresh_missing_requested_connectors(
    invocation: &ToolInvocation,
    auth: Option<&CodexAuth>,
    expected_connector_ids: &[String],
    tool_id: &str,
) -> Option<Vec<AppInfo>> {
    if expected_connector_ids.is_empty() {
        return Some(Vec::new());
    }

    let mcp_tools = invocation.list_mcp_tools().await;
    let accessible_connectors = connectors::with_app_enabled_state(
        connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
        invocation.config(),
    );
    if all_requested_connectors_picked_up(expected_connector_ids, &accessible_connectors) {
        return Some(accessible_connectors);
    }

    match invocation.hard_refresh_codex_apps_tools_cache().await {
        Ok(mcp_tools) => {
            let accessible_connectors = connectors::with_app_enabled_state(
                connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
                invocation.config(),
            );
            connectors::refresh_accessible_connectors_cache_from_mcp_tools(
                invocation.config(),
                auth,
                &mcp_tools,
            );
            Some(accessible_connectors)
        }
        Err(err) => {
            warn!(
                "failed to refresh codex apps tools cache after plugin install request for {tool_id}: {err:#}"
            );
            None
        }
    }
}

fn verified_plugin_install_completed(
    tool_id: &str,
    config: &Config,
    plugins_manager: &PluginsManager,
) -> bool {
    let plugins_input = config.plugins_config_input();
    plugins_manager
        .list_marketplaces_for_config(&plugins_input, &[])
        .ok()
        .into_iter()
        .flat_map(|outcome| outcome.marketplaces)
        .flat_map(|marketplace| marketplace.plugins.into_iter())
        .any(|plugin| plugin.id == tool_id && plugin.installed)
}
