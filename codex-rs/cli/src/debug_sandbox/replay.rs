use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use codex_config::NetworkConstraints;
use codex_network_proxy::NetworkProxyConfig;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SandboxReplayPayload {
    pub(super) permission_profile: PermissionProfile,
    pub(super) network_proxy: Option<SandboxReplayNetworkProxy>,
    pub(super) managed_network_requirements_enabled: bool,
    pub(super) sandbox_cwd: AbsolutePathBuf,
    pub(super) codex_home: AbsolutePathBuf,
    pub(super) env: HashMap<String, String>,
    pub(super) codex_linux_sandbox_exe: Option<PathBuf>,
    pub(super) use_legacy_landlock: bool,
    pub(super) windows_sandbox_level: WindowsSandboxLevel,
    pub(super) windows_sandbox_private_desktop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SandboxReplayNetworkProxy {
    pub(super) config: NetworkProxyConfig,
    pub(super) requirements: Option<NetworkConstraints>,
}

pub(super) fn parse_sandbox_replay_payload(json: &str) -> anyhow::Result<SandboxReplayPayload> {
    serde_json::from_str(json).context("failed to parse sandbox replay JSON")
}
