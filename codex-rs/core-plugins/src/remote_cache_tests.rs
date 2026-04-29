use super::*;
use anyhow::Result;
use codex_login::CodexAuth;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

#[tokio::test]
async fn clear_remote_plugin_cache_removes_only_remote_namespaces() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_cached_plugin(&codex_home, REMOTE_GLOBAL_MARKETPLACE_NAME, "linear")?;
    write_cached_plugin(
        &codex_home,
        REMOTE_WORKSPACE_MARKETPLACE_NAME,
        "workspace-tool",
    )?;
    write_cached_plugin(&codex_home, "openai-curated", "gmail")?;
    write_cached_plugin(&codex_home, "debug", "sample")?;
    let plugin_data = codex_home.path().join("plugins/data/linear-chatgpt-global");
    std::fs::create_dir_all(&plugin_data)?;

    clear_remote_plugin_cache(codex_home.path().to_path_buf()).await?;

    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-global")
            .exists()
    );
    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-workspace")
            .exists()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/openai-curated/gmail")
            .is_dir()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/debug/sample")
            .is_dir()
    );
    assert!(plugin_data.is_dir());
    Ok(())
}

#[tokio::test]
async fn prune_remote_plugin_cache_preserves_current_account_installed_plugins() -> Result<()> {
    let codex_home = TempDir::new()?;
    let server = MockServer::start().await;
    mount_empty_directory_plugins(&server).await;
    mount_installed_plugins(
        &server,
        "GLOBAL",
        json!([remote_plugin_item(
            "plugins~Plugin_linear",
            "linear",
            "GLOBAL",
            true
        )]),
    )
    .await;
    mount_installed_plugins(
        &server,
        "WORKSPACE",
        json!([remote_plugin_item(
            "plugins~Plugin_workspace",
            "workspace-tool",
            "WORKSPACE",
            false
        )]),
    )
    .await;

    write_cached_plugin(&codex_home, REMOTE_GLOBAL_MARKETPLACE_NAME, "linear")?;
    write_cached_plugin(
        &codex_home,
        REMOTE_GLOBAL_MARKETPLACE_NAME,
        "plugins~Plugin_linear",
    )?;
    write_cached_plugin(&codex_home, REMOTE_GLOBAL_MARKETPLACE_NAME, "stale-global")?;
    write_cached_plugin(
        &codex_home,
        REMOTE_WORKSPACE_MARKETPLACE_NAME,
        "workspace-tool",
    )?;
    write_cached_plugin(
        &codex_home,
        REMOTE_WORKSPACE_MARKETPLACE_NAME,
        "stale-workspace",
    )?;
    write_cached_plugin(&codex_home, "openai-curated", "gmail")?;

    let config = RemotePluginServiceConfig {
        chatgpt_base_url: format!("{}/backend-api", server.uri()),
    };
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    prune_remote_plugin_cache_for_current_auth(
        &config,
        Some(&auth),
        codex_home.path().to_path_buf(),
    )
    .await?;

    assert!(
        codex_home
            .path()
            .join("plugins/cache/chatgpt-global/linear")
            .is_dir()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/chatgpt-global/plugins~Plugin_linear")
            .is_dir()
    );
    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-global/stale-global")
            .exists()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/chatgpt-workspace/workspace-tool")
            .is_dir()
    );
    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-workspace/stale-workspace")
            .exists()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/openai-curated/gmail")
            .is_dir()
    );

    let requests = server.received_requests().await.unwrap_or_default();
    let requested_paths = requests
        .iter()
        .map(|request| {
            let query = request.url.query().unwrap_or_default();
            format!("{}?{query}", request.url.path())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        requested_paths
            .iter()
            .filter(|path| path.starts_with("/backend-api/ps/plugins/installed?"))
            .count(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn prune_remote_plugin_cache_clears_all_remote_cache_when_account_fetch_fails() -> Result<()>
{
    let codex_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_cached_plugin(&codex_home, REMOTE_GLOBAL_MARKETPLACE_NAME, "linear")?;
    write_cached_plugin(
        &codex_home,
        REMOTE_WORKSPACE_MARKETPLACE_NAME,
        "workspace-tool",
    )?;
    write_cached_plugin(&codex_home, "openai-curated", "gmail")?;

    let config = RemotePluginServiceConfig {
        chatgpt_base_url: format!("{}/backend-api", server.uri()),
    };
    let auth = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    prune_remote_plugin_cache_for_current_auth(
        &config,
        Some(&auth),
        codex_home.path().to_path_buf(),
    )
    .await?;

    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-global")
            .exists()
    );
    assert!(
        !codex_home
            .path()
            .join("plugins/cache/chatgpt-workspace")
            .exists()
    );
    assert!(
        codex_home
            .path()
            .join("plugins/cache/openai-curated/gmail")
            .is_dir()
    );
    Ok(())
}

async fn mount_empty_directory_plugins(server: &MockServer) {
    for scope in ["GLOBAL", "WORKSPACE"] {
        Mock::given(method("GET"))
            .and(path("/backend-api/ps/plugins/list"))
            .and(query_param("scope", scope))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "plugins": [],
                "pagination": {
                    "next_page_token": null
                }
            })))
            .mount(server)
            .await;
    }
}

async fn mount_installed_plugins(
    server: &MockServer,
    scope: &'static str,
    plugins: serde_json::Value,
) {
    Mock::given(method("GET"))
        .and(path("/backend-api/ps/plugins/installed"))
        .and(query_param("scope", scope))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "plugins": plugins,
            "pagination": {
                "next_page_token": null
            }
        })))
        .mount(server)
        .await;
}

fn remote_plugin_item(
    remote_plugin_id: &str,
    plugin_name: &str,
    scope: &str,
    enabled: bool,
) -> serde_json::Value {
    json!({
        "id": remote_plugin_id,
        "name": plugin_name,
        "scope": scope,
        "installation_policy": "AVAILABLE",
        "authentication_policy": "ON_USE",
        "enabled": enabled,
        "release": {
            "version": "1.0.0",
            "display_name": plugin_name,
            "description": plugin_name,
            "interface": {},
            "app_ids": [],
            "skills": []
        }
    })
}

fn write_cached_plugin(
    codex_home: &TempDir,
    marketplace_name: &str,
    plugin_name: &str,
) -> Result<()> {
    let plugin_root = codex_home
        .path()
        .join("plugins/cache")
        .join(marketplace_name)
        .join(plugin_name)
        .join("1.0.0/.codex-plugin");
    std::fs::create_dir_all(&plugin_root)?;
    std::fs::write(
        plugin_root.join("plugin.json"),
        format!(r#"{{"name":"{plugin_name}","version":"1.0.0"}}"#),
    )?;
    Ok(())
}
