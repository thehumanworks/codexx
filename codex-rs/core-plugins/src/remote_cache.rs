use crate::remote::REMOTE_GLOBAL_MARKETPLACE_NAME;
use crate::remote::REMOTE_WORKSPACE_MARKETPLACE_NAME;
use crate::remote::RemoteMarketplace;
use crate::remote::RemotePluginCatalogError;
use crate::remote::RemotePluginServiceConfig;
use crate::remote::fetch_remote_marketplaces;
use crate::store::PLUGINS_CACHE_DIR;
use codex_login::CodexAuth;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tracing::warn;

const REMOTE_MARKETPLACE_NAMES: [&str; 2] = [
    REMOTE_GLOBAL_MARKETPLACE_NAME,
    REMOTE_WORKSPACE_MARKETPLACE_NAME,
];

#[derive(Debug, Default)]
struct RemotePluginCacheRetainSet {
    plugin_names_by_marketplace: BTreeMap<String, BTreeSet<String>>,
}

impl RemotePluginCacheRetainSet {
    fn from_marketplaces(marketplaces: Vec<RemoteMarketplace>) -> Self {
        let mut retain = Self::default();
        for marketplace in marketplaces {
            if !REMOTE_MARKETPLACE_NAMES.contains(&marketplace.name.as_str()) {
                continue;
            }
            let plugin_names = retain
                .plugin_names_by_marketplace
                .entry(marketplace.name)
                .or_default();
            for plugin in marketplace.plugins {
                if plugin.installed {
                    plugin_names.insert(plugin.name);
                    plugin_names.insert(plugin.id);
                }
            }
        }
        retain
    }

    fn contains(&self, marketplace_name: &str, plugin_cache_name: &str) -> bool {
        self.plugin_names_by_marketplace
            .get(marketplace_name)
            .is_some_and(|plugin_names| plugin_names.contains(plugin_cache_name))
    }
}

/// Remove all locally cached remote plugin bundles.
///
/// This is used when there is no authenticated ChatGPT account whose installed
/// plugin set can be trusted, such as logout or API-key login.
pub async fn clear_remote_plugin_cache(
    codex_home: PathBuf,
) -> Result<(), RemotePluginCatalogError> {
    run_cache_mutation("remote plugin cache clear", move || {
        clear_remote_plugin_cache_blocking(codex_home.as_path())
    })
    .await
}

/// Keep only remote plugin cache entries that belong to the authenticated account.
///
/// Disabled remote plugins are retained because the backend still reports them
/// as installed; disabled state controls availability, not local cache ownership.
/// If the account cannot be read, all remote plugin cache entries are removed so
/// stale bundles from a previous account cannot stay visible locally.
pub async fn prune_remote_plugin_cache_for_current_auth(
    config: &RemotePluginServiceConfig,
    auth: Option<&CodexAuth>,
    codex_home: PathBuf,
) -> Result<(), RemotePluginCatalogError> {
    let marketplaces = match fetch_remote_marketplaces(config, auth).await {
        Ok(marketplaces) => marketplaces,
        Err(err) => {
            warn!(
                error = %err,
                "failed to fetch account remote plugin state; clearing all remote plugin cache entries"
            );
            return clear_remote_plugin_cache(codex_home).await;
        }
    };
    let retain = RemotePluginCacheRetainSet::from_marketplaces(marketplaces);
    run_cache_mutation("remote plugin cache prune", move || {
        prune_remote_plugin_cache_blocking(codex_home.as_path(), &retain)
    })
    .await
}

async fn run_cache_mutation<F>(
    context: &'static str,
    mutation: F,
) -> Result<(), RemotePluginCatalogError>
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    tokio::task::spawn_blocking(mutation)
        .await
        .map_err(|err| {
            RemotePluginCatalogError::CacheRemove(format!("failed to join {context} task: {err}"))
        })?
        .map_err(RemotePluginCatalogError::CacheRemove)
}

fn clear_remote_plugin_cache_blocking(codex_home: &Path) -> Result<(), String> {
    for marketplace_name in REMOTE_MARKETPLACE_NAMES {
        remove_path_if_exists(&remote_marketplace_cache_root(codex_home, marketplace_name))?;
    }
    Ok(())
}

fn prune_remote_plugin_cache_blocking(
    codex_home: &Path,
    retain: &RemotePluginCacheRetainSet,
) -> Result<(), String> {
    for marketplace_name in REMOTE_MARKETPLACE_NAMES {
        let marketplace_root = remote_marketplace_cache_root(codex_home, marketplace_name);
        if !marketplace_root.exists() {
            continue;
        }
        if !marketplace_root.is_dir() {
            remove_path_if_exists(&marketplace_root)?;
            continue;
        }

        let entries = fs::read_dir(&marketplace_root).map_err(|err| {
            format!(
                "failed to read remote plugin cache namespace {}: {err}",
                marketplace_root.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                format!(
                    "failed to enumerate remote plugin cache namespace {}: {err}",
                    marketplace_root.display()
                )
            })?;
            let plugin_cache_name = entry.file_name();
            let Some(plugin_cache_name) = plugin_cache_name.to_str() else {
                remove_path_if_exists(&entry.path())?;
                continue;
            };
            if !retain.contains(marketplace_name, plugin_cache_name) {
                remove_path_if_exists(&entry.path())?;
            }
        }
    }
    Ok(())
}

fn remote_marketplace_cache_root(codex_home: &Path, marketplace_name: &str) -> PathBuf {
    codex_home.join(PLUGINS_CACHE_DIR).join(marketplace_name)
}

fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let result = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|err| {
        format!(
            "failed to remove remote plugin cache entry {}: {err}",
            path.display()
        )
    })
}

#[cfg(test)]
#[path = "remote_cache_tests.rs"]
mod tests;
