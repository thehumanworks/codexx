use codex_client::is_allowed_chatgpt_host;
use codex_config::McpServerProvenance;
use url::Host;
use url::Url;

use super::McpConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HostOwnedCodexAppsMcpEndpoint {
    url: trusted_codex_apps_mcp_url::TrustedCodexAppsMcpUrl,
    provenance: McpServerProvenance,
}

impl HostOwnedCodexAppsMcpEndpoint {
    fn new(url: trusted_codex_apps_mcp_url::TrustedCodexAppsMcpUrl) -> Self {
        Self {
            url,
            provenance: McpServerProvenance::HostOwnedCodexApps,
        }
    }

    pub(super) fn into_parts(self) -> (String, McpServerProvenance) {
        (self.url.into_string(), self.provenance)
    }

    #[cfg(test)]
    pub(super) fn url(&self) -> &str {
        self.url.as_str()
    }

    #[cfg(test)]
    pub(super) fn provenance(&self) -> McpServerProvenance {
        self.provenance
    }
}

/// Builds the only endpoint allowed to receive host-owned Codex Apps provenance.
pub(super) fn host_owned_codex_apps_mcp_endpoint(
    config: &McpConfig,
) -> Result<HostOwnedCodexAppsMcpEndpoint, String> {
    // HostOwnedCodexApps gates first-party connector behavior, including
    // privileged file upload handling. Keep the trusted URL check and the
    // provenance grant together so a config-derived URL cannot receive the
    // host-owned marker without passing this audit point.
    let url = trusted_codex_apps_mcp_url::from_base_url(
        &config.chatgpt_base_url,
        config.apps_mcp_path_override.as_deref(),
    )?;
    Ok(HostOwnedCodexAppsMcpEndpoint::new(url))
}

mod trusted_codex_apps_mcp_url {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct TrustedCodexAppsMcpUrl(String);

    impl TrustedCodexAppsMcpUrl {
        fn new(url: String) -> Result<Self, String> {
            let parsed_url = Url::parse(&url)
                .map_err(|err| format!("invalid Codex Apps MCP URL `{url}`: {err}"))?;
            validate_url(&parsed_url, &url, "URL")?;
            Ok(Self(url))
        }

        pub(super) fn into_string(self) -> String {
            self.0
        }

        #[cfg(test)]
        pub(super) fn as_str(&self) -> &str {
            &self.0
        }
    }

    pub(super) fn from_base_url(
        base_url: &str,
        apps_mcp_path_override: Option<&str>,
    ) -> Result<TrustedCodexAppsMcpUrl, String> {
        let base_url = base_url.trim_end_matches('/');
        let parsed_base_url = Url::parse(base_url)
            .map_err(|err| format!("invalid Codex Apps MCP base URL `{base_url}`: {err}"))?;
        validate_url(&parsed_base_url, base_url, "base URL")?;

        let mut base_url = base_url.to_string();
        if is_allowed_chatgpt_host_url(&parsed_base_url.host())
            && !parsed_base_url.path().contains("/backend-api")
        {
            base_url = format!("{base_url}/backend-api");
        }
        let (base_url, default_path) = if base_url.contains("/backend-api") {
            (base_url, "wham/apps")
        } else if base_url.contains("/api/codex") {
            (base_url, "apps")
        } else {
            (format!("{base_url}/api/codex"), "apps")
        };
        let path = apps_mcp_path_override
            .unwrap_or(default_path)
            .trim_start_matches('/');
        TrustedCodexAppsMcpUrl::new(format!("{base_url}/{path}"))
    }

    fn validate_url(url: &Url, original_url: &str, label: &str) -> Result<(), String> {
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(format!(
                "invalid Codex Apps MCP {label} `{original_url}`; expected a URL without credentials, query, or fragment"
            ));
        }

        let scheme = url.scheme();
        let host = url.host();
        let valid_first_party_url =
            scheme == "https" && url.port().is_none() && is_allowed_chatgpt_host_url(&host);
        let valid_local_url =
            cfg!(debug_assertions) && matches!(scheme, "http" | "https") && is_localhost(&host);
        if valid_first_party_url || valid_local_url {
            Ok(())
        } else {
            Err(format!(
                "invalid Codex Apps MCP {label} `{original_url}`; expected an HTTPS URL for chatgpt.com, chat.openai.com, or chatgpt-staging.com"
            ))
        }
    }

    fn is_allowed_chatgpt_host_url(host: &Option<Host<&str>>) -> bool {
        let Some(Host::Domain(host)) = host else {
            return false;
        };
        is_allowed_chatgpt_host(host)
    }

    fn is_localhost(host: &Option<Host<&str>>) -> bool {
        match host {
            Some(Host::Domain(host)) => *host == "localhost",
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "codex_apps_endpoint_tests.rs"]
mod tests;
