use super::*;
use pretty_assertions::assert_eq;

#[test]
fn trusted_url_keeps_existing_paths() {
    assert_eq!(
        trusted_codex_apps_mcp_url::from_base_url(
            "https://chatgpt.com/backend-api",
            /*apps_mcp_path_override*/ None,
        )
        .expect("trusted ChatGPT URL should build")
        .as_str(),
        "https://chatgpt.com/backend-api/wham/apps"
    );
    assert_eq!(
        trusted_codex_apps_mcp_url::from_base_url(
            "https://chat.openai.com",
            /*apps_mcp_path_override*/ None,
        )
        .expect("trusted legacy ChatGPT URL should build")
        .as_str(),
        "https://chat.openai.com/backend-api/wham/apps"
    );
    assert_eq!(
        trusted_codex_apps_mcp_url::from_base_url(
            "http://localhost:8080/api/codex",
            /*apps_mcp_path_override*/ None,
        )
        .expect("local debug URL should build")
        .as_str(),
        "http://localhost:8080/api/codex/apps"
    );
    assert_eq!(
        trusted_codex_apps_mcp_url::from_base_url(
            "http://localhost:8080",
            /*apps_mcp_path_override*/ None,
        )
        .expect("local debug URL should build")
        .as_str(),
        "http://localhost:8080/api/codex/apps"
    );
}

#[test]
fn trusted_url_rejects_untrusted_base_urls() {
    for base_url in [
        "http://chatgpt.com/backend-api",
        "https://example.com/backend-api",
        "https://chatgpt.com.evil.example/backend-api",
        "https://evilchatgpt.com/backend-api",
        "https://foo.chat.openai.com/backend-api",
        "https://chatgpt.com:4443/backend-api",
        "https://user:pass@chatgpt.com/backend-api",
        "https://chatgpt.com/backend-api?token=secret",
    ] {
        let err = trusted_codex_apps_mcp_url::from_base_url(
            base_url, /*apps_mcp_path_override*/ None,
        )
        .expect_err("untrusted URL should be rejected");

        assert!(
            err.starts_with("invalid Codex Apps MCP base URL"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn trusted_url_rejects_override_that_adds_query_or_fragment() {
    for path_override in ["custom/mcp?token=secret", "custom/mcp#fragment"] {
        let err =
            trusted_codex_apps_mcp_url::from_base_url("https://chatgpt.com", Some(path_override))
                .expect_err("untrusted final URL should be rejected");

        assert!(
            err.starts_with("invalid Codex Apps MCP URL"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn endpoint_pairs_trusted_url_with_provenance() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let config = crate::mcp::tests::test_mcp_config(codex_home.path().to_path_buf());
    let endpoint =
        host_owned_codex_apps_mcp_endpoint(&config).expect("trusted ChatGPT URL should build");

    assert_eq!(
        (endpoint.url(), endpoint.provenance()),
        (
            "https://chatgpt.com/backend-api/wham/apps",
            McpServerProvenance::HostOwnedCodexApps,
        )
    );
}
