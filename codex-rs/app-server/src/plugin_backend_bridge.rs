use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::Uri;
use axum::http::header::AUTHORIZATION;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::any;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use codex_config::config_toml::PluginBackendBridgeConfigToml;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::default_client::build_reqwest_client;
use rand::RngCore as _;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::info;

const STATE_FILE_PREFIX: &str = "plugin-backend-bridge.";
const STATE_FILE_SUFFIX: &str = ".json";

#[derive(Clone)]
struct PluginBackendBridgeState {
    auth_manager: Arc<AuthManager>,
    chatgpt_base_url: String,
    client: reqwest::Client,
    backend_path_prefix: String,
    local_path_prefix: String,
    token: String,
}

#[derive(Deserialize, Serialize)]
struct PluginBackendBridgeStateFile {
    base_url: String,
    token: String,
}

pub(crate) async fn start_plugin_backend_bridges(
    codex_home: PathBuf,
    chatgpt_base_url: String,
    auth_manager: Arc<AuthManager>,
    bridge_configs: HashMap<String, PluginBackendBridgeConfigToml>,
    shutdown_token: CancellationToken,
) -> io::Result<Vec<JoinHandle<()>>> {
    let bridge_shutdown_token = shutdown_token.child_token();
    let mut bridge_configs = bridge_configs.into_iter().collect::<Vec<_>>();
    bridge_configs.sort_by(|(left_id, _), (right_id, _)| left_id.cmp(right_id));
    let mut handles = Vec::with_capacity(bridge_configs.len());
    for (bridge_id, config) in bridge_configs {
        match start_plugin_backend_bridge(
            codex_home.clone(),
            chatgpt_base_url.clone(),
            auth_manager.clone(),
            bridge_id,
            config,
            bridge_shutdown_token.clone(),
        )
        .await
        {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                bridge_shutdown_token.cancel();
                for handle in handles {
                    let _ = handle.await;
                }
                return Err(err);
            }
        }
    }
    Ok(handles)
}

async fn start_plugin_backend_bridge(
    codex_home: PathBuf,
    chatgpt_base_url: String,
    auth_manager: Arc<AuthManager>,
    bridge_id: String,
    config: PluginBackendBridgeConfigToml,
    shutdown_token: CancellationToken,
) -> io::Result<JoinHandle<()>> {
    validate_bridge_id(&bridge_id)?;
    validate_path_prefix("local_path_prefix", &config.local_path_prefix)?;
    validate_path_prefix("backend_path_prefix", &config.backend_path_prefix)?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let local_addr = listener.local_addr()?;
    let state = PluginBackendBridgeStateFile {
        base_url: format!("http://{local_addr}"),
        token: random_bridge_token(),
    };
    let state_file_path = codex_home.join(state_file_name(&bridge_id));

    write_state_file(&codex_home, &state_file_path, &state).await?;

    let router = Router::new()
        .fallback(any(handle_plugin_backend_bridge_request))
        .with_state(PluginBackendBridgeState {
            auth_manager,
            chatgpt_base_url,
            client: build_reqwest_client(),
            backend_path_prefix: config.backend_path_prefix,
            local_path_prefix: config.local_path_prefix,
            token: state.token.clone(),
        });
    let server = axum::serve(listener, router).with_graceful_shutdown({
        let shutdown_token = shutdown_token.clone();
        async move {
            shutdown_token.cancelled().await;
        }
    });

    info!(
        bridge_id,
        base_url = state.base_url,
        state_file_path = %state_file_path.display(),
        "plugin backend bridge listening"
    );
    Ok(tokio::spawn(async move {
        if let Err(err) = server.await {
            error!(bridge_id, "plugin backend bridge failed: {err}");
        }
        if let Err(err) = remove_state_file_if_owned(&state_file_path, &state).await {
            error!(
                bridge_id,
                state_file_path = %state_file_path.display(),
                "failed to remove plugin backend bridge state file: {err}"
            );
        }
        info!(bridge_id, "plugin backend bridge shutting down");
    }))
}

fn state_file_name(bridge_id: &str) -> String {
    format!("{STATE_FILE_PREFIX}{bridge_id}{STATE_FILE_SUFFIX}")
}

fn validate_bridge_id(bridge_id: &str) -> io::Result<()> {
    if bridge_id.is_empty()
        || !bridge_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "plugin backend bridge ids may contain only ASCII letters, digits, `_`, and `-`",
        ));
    }
    Ok(())
}

fn validate_path_prefix(field_name: &str, path_prefix: &str) -> io::Result<()> {
    if !path_prefix.starts_with('/') || !path_prefix.ends_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("plugin backend bridge {field_name} must start and end with `/`"),
        ));
    }
    Ok(())
}

async fn write_state_file(
    codex_home: &Path,
    state_file_path: &Path,
    state: &PluginBackendBridgeStateFile,
) -> io::Result<()> {
    fs::create_dir_all(codex_home).await?;
    let state_file = serde_json::to_vec(state).map_err(io::Error::other)?;
    fs::write(state_file_path, state_file).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(state_file_path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

async fn remove_state_file_if_owned(
    state_file_path: &Path,
    expected_state: &PluginBackendBridgeStateFile,
) -> io::Result<()> {
    let payload = match fs::read(state_file_path).await {
        Ok(payload) => payload,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let current_state = serde_json::from_slice::<PluginBackendBridgeStateFile>(&payload)
        .map_err(io::Error::other)?;
    if current_state.base_url == expected_state.base_url
        && current_state.token == expected_state.token
    {
        fs::remove_file(state_file_path).await?;
    }
    Ok(())
}

async fn handle_plugin_backend_bridge_request(
    State(state): State<PluginBackendBridgeState>,
    request: Request,
) -> Response {
    if !has_valid_bridge_token(request.headers(), &state.token) {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "Unauthorized plugin backend bridge request.",
        );
    }
    if request.method() != Method::GET && request.method() != Method::POST {
        return json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Unsupported plugin backend bridge method.",
        );
    }
    let Some(backend_path) = build_backend_path(
        request.uri(),
        &state.local_path_prefix,
        &state.backend_path_prefix,
    ) else {
        return json_error(StatusCode::NOT_FOUND, "Unknown plugin backend bridge path.");
    };
    let method = request.method().clone();
    let body = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
        Ok(body) => body,
        Err(err) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to read plugin backend bridge request body: {err}"),
            );
        }
    };

    match forward_to_backend(&state, method, backend_path, body).await {
        Ok(response) => response,
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Plugin backend bridge request failed: {err}"),
        ),
    }
}

fn has_valid_bridge_token(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(&format!("Bearer {token}"))
}

fn build_backend_path(
    uri: &Uri,
    local_path_prefix: &str,
    backend_path_prefix: &str,
) -> Option<String> {
    let path = uri.path();
    if !path.starts_with(local_path_prefix) {
        return None;
    }
    let mut backend_path = format!(
        "{}{}",
        backend_path_prefix.trim_end_matches('/'),
        &path[local_path_prefix.len().saturating_sub(1)..]
    );
    if let Some(query) = uri.query() {
        backend_path.push('?');
        backend_path.push_str(query);
    }
    Some(backend_path)
}

fn backend_url(
    chatgpt_base_url: &str,
    backend_path: &str,
    backend_path_prefix: &str,
) -> io::Result<reqwest::Url> {
    let base_url = chatgpt_base_url.trim_end_matches('/');
    let backend_base_url = base_url.strip_suffix("/backend-api").unwrap_or(base_url);
    let url = reqwest::Url::parse(&format!(
        "{}/{}",
        backend_base_url,
        backend_path.trim_start_matches('/')
    ))
    .map_err(io::Error::other)?;
    if !url.path().starts_with(backend_path_prefix) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Plugin backend bridge request escaped its configured backend path prefix.",
        ));
    }
    Ok(url)
}

async fn forward_to_backend(
    state: &PluginBackendBridgeState,
    method: Method,
    backend_path: String,
    body: axum::body::Bytes,
) -> io::Result<Response> {
    let backend_url = backend_url(
        &state.chatgpt_base_url,
        &backend_path,
        &state.backend_path_prefix,
    )?;
    let mut auth_recovery = state.auth_manager.unauthorized_recovery();
    loop {
        let auth = bridge_auth(&state.auth_manager).await?;
        let response =
            send_backend_request(state, &auth, &method, backend_url.clone(), body.clone())
                .await
                .map_err(io::Error::other)?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED || !auth_recovery.has_next() {
            return response_from_reqwest(response).await;
        }
        auth_recovery.next().await.map_err(io::Error::other)?;
    }
}

async fn bridge_auth(auth_manager: &Arc<AuthManager>) -> io::Result<CodexAuth> {
    let Some(auth) = auth_manager.auth().await else {
        return Err(io::Error::other(
            "Sign in to ChatGPT in Codex to use this bridge.",
        ));
    };
    if !auth.uses_codex_backend() {
        return Err(io::Error::other(
            "ChatGPT authentication is required to use this bridge.",
        ));
    }
    Ok(auth)
}

async fn send_backend_request(
    state: &PluginBackendBridgeState,
    auth: &CodexAuth,
    method: &Method,
    backend_url: reqwest::Url,
    body: axum::body::Bytes,
) -> reqwest::Result<reqwest::Response> {
    let mut headers = codex_model_provider::auth_provider_from_auth(auth).to_auth_headers();
    if !body.is_empty() {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    state
        .client
        .request(method.clone(), backend_url)
        .headers(headers)
        .body(body)
        .send()
        .await
}

async fn response_from_reqwest(response: reqwest::Response) -> io::Result<Response> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let body = response.bytes().await.map_err(io::Error::other)?;
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body)).map_err(io::Error::other)
}

fn json_error(status: StatusCode, detail: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "detail": detail,
        })),
    )
        .into_response()
}

fn random_bridge_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::to_bytes;
    use axum::routing::any;
    use codex_config::types::AuthCredentialsStoreMode;
    use codex_core::test_support::auth_manager_from_auth;
    use codex_login::ExternalAuth;
    use codex_login::ExternalAuthRefreshContext;
    use codex_login::ExternalAuthTokens;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn maps_only_configured_local_paths_to_backend_paths() {
        let uri: Uri = "/local/example/jobs/abc?include=true"
            .parse()
            .expect("valid uri");
        assert_eq!(
            build_backend_path(&uri, "/local/example/", "/api/codex/example/"),
            Some("/api/codex/example/jobs/abc?include=true".to_string())
        );
        let unrelated_uri: Uri = "/not-example".parse().expect("valid uri");
        assert_eq!(
            build_backend_path(&unrelated_uri, "/local/example/", "/api/codex/example/"),
            None
        );
    }

    #[test]
    fn strips_backend_api_before_building_backend_url() {
        assert_eq!(
            backend_url(
                "https://chatgpt.com/backend-api/",
                "/api/codex/example/auth-test",
                "/api/codex/example/",
            )
            .expect("backend url should build")
            .as_str(),
            "https://chatgpt.com/api/codex/example/auth-test"
        );
        assert_eq!(
            backend_url(
                "http://127.0.0.1:8061",
                "/api/codex/example/auth-test",
                "/api/codex/example/",
            )
            .expect("backend url should build")
            .as_str(),
            "http://127.0.0.1:8061/api/codex/example/auth-test"
        );
    }

    #[test]
    fn rejects_backend_urls_that_escape_the_configured_prefix_after_normalization() {
        assert!(
            backend_url(
                "https://chatgpt.com/backend-api/",
                "/api/codex/example/../admin",
                "/api/codex/example/",
            )
            .is_err()
        );
        assert!(
            backend_url(
                "https://chatgpt.com/backend-api/",
                "/api/codex/example/%2e%2e/admin",
                "/api/codex/example/",
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn rejects_requests_without_the_bridge_token() {
        let codex_home = TempDir::new().expect("temp dir should exist");
        let state = PluginBackendBridgeState {
            auth_manager: AuthManager::shared(
                codex_home.path().to_path_buf(),
                /*enable_codex_api_key_env*/ false,
                codex_config::types::AuthCredentialsStoreMode::Ephemeral,
                /*chatgpt_base_url*/ None,
            )
            .await,
            chatgpt_base_url: "https://chatgpt.com/backend-api/".to_string(),
            client: build_reqwest_client(),
            backend_path_prefix: "/api/codex/example/".to_string(),
            local_path_prefix: "/local/example/".to_string(),
            token: "bridge-token".to_string(),
        };
        let request = Request::builder()
            .uri("/local/example/auth-test")
            .body(Body::empty())
            .expect("request should build");

        let response = handle_plugin_backend_bridge_request(State(state), request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).expect("valid json"),
            serde_json::json!({"detail": "Unauthorized plugin backend bridge request."})
        );
    }

    #[tokio::test]
    async fn removes_only_owned_state_files() {
        let codex_home = TempDir::new().expect("temp dir should exist");
        let state_file_path = codex_home.path().join(state_file_name("example"));
        let owned_state = PluginBackendBridgeStateFile {
            base_url: "http://127.0.0.1:1111".to_string(),
            token: "owned".to_string(),
        };
        let replacement_state = PluginBackendBridgeStateFile {
            base_url: "http://127.0.0.1:2222".to_string(),
            token: "replacement".to_string(),
        };

        write_state_file(codex_home.path(), &state_file_path, &replacement_state)
            .await
            .expect("state file should write");
        remove_state_file_if_owned(&state_file_path, &owned_state)
            .await
            .expect("foreign state should be preserved");
        assert!(state_file_path.exists());

        write_state_file(codex_home.path(), &state_file_path, &owned_state)
            .await
            .expect("state file should rewrite");
        remove_state_file_if_owned(&state_file_path, &owned_state)
            .await
            .expect("owned state should be removed");
        assert!(!state_file_path.exists());
    }

    #[tokio::test]
    async fn rolls_back_started_bridges_when_later_config_fails() {
        let codex_home = TempDir::new().expect("temp dir should exist");
        let valid_bridge_id = "a-valid".to_string();
        let valid_state_file_path = codex_home.path().join(state_file_name(&valid_bridge_id));
        let bridge_configs = HashMap::from([
            (
                valid_bridge_id,
                PluginBackendBridgeConfigToml {
                    local_path_prefix: "/local/example/".to_string(),
                    backend_path_prefix: "/api/codex/example/".to_string(),
                },
            ),
            (
                "b-invalid".to_string(),
                PluginBackendBridgeConfigToml {
                    local_path_prefix: "missing-leading-slash/".to_string(),
                    backend_path_prefix: "/api/codex/example/".to_string(),
                },
            ),
        ]);

        let result = start_plugin_backend_bridges(
            codex_home.path().to_path_buf(),
            "https://chatgpt.com/backend-api/".to_string(),
            AuthManager::shared(
                codex_home.path().to_path_buf(),
                /*enable_codex_api_key_env*/ false,
                AuthCredentialsStoreMode::Ephemeral,
                /*chatgpt_base_url*/ None,
            )
            .await,
            bridge_configs,
            CancellationToken::new(),
        )
        .await;

        assert!(result.is_err());
        assert!(!valid_state_file_path.exists());
    }

    #[tokio::test]
    async fn forwards_host_auth_headers_to_the_backend() {
        let captured_headers = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
        let (base_url, server_handle) = spawn_test_backend({
            let captured_headers = Arc::clone(&captured_headers);
            move |headers| {
                captured_headers
                    .lock()
                    .expect("capture lock should not be poisoned")
                    .push(headers);
                StatusCode::OK
            }
        })
        .await;
        let state = PluginBackendBridgeState {
            auth_manager: auth_manager_from_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing()),
            chatgpt_base_url: base_url,
            client: build_reqwest_client(),
            backend_path_prefix: "/api/codex/example/".to_string(),
            local_path_prefix: "/local/example/".to_string(),
            token: "bridge-token".to_string(),
        };

        let response = forward_to_backend(
            &state,
            Method::GET,
            "/api/codex/example/auth-test".to_string(),
            axum::body::Bytes::new(),
        )
        .await
        .expect("forward should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let headers = captured_headers
            .lock()
            .expect("capture lock should not be poisoned");
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers[0]
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer Access Token")
        );
        assert_eq!(
            headers[0]
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok()),
            Some("account_id")
        );
        server_handle.abort();
    }

    #[tokio::test]
    async fn retries_once_with_refreshed_external_chatgpt_auth_after_401() {
        let codex_home = TempDir::new().expect("temp dir should exist");
        let stale_token = fake_jwt("stale-token@example.com");
        let fresh_token = fake_jwt("fresh-token@example.com");
        codex_login::auth::login_with_chatgpt_auth_tokens(
            codex_home.path(),
            &stale_token,
            "account_id",
            /*chatgpt_plan_type*/ None,
        )
        .expect("external chatgpt auth should save");
        let auth_manager = AuthManager::shared(
            codex_home.path().to_path_buf(),
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::Ephemeral,
            /*chatgpt_base_url*/ None,
        )
        .await;
        auth_manager.set_external_auth(Arc::new(TestExternalAuth {
            fresh_token: fresh_token.clone(),
        }));

        let captured_headers = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
        let (base_url, server_handle) = spawn_test_backend({
            let captured_headers = Arc::clone(&captured_headers);
            move |headers| {
                let mut captured = captured_headers
                    .lock()
                    .expect("capture lock should not be poisoned");
                captured.push(headers);
                if captured.len() == 1 {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::OK
                }
            }
        })
        .await;
        let state = PluginBackendBridgeState {
            auth_manager,
            chatgpt_base_url: base_url,
            client: build_reqwest_client(),
            backend_path_prefix: "/api/codex/example/".to_string(),
            local_path_prefix: "/local/example/".to_string(),
            token: "bridge-token".to_string(),
        };

        let response = forward_to_backend(
            &state,
            Method::GET,
            "/api/codex/example/auth-test".to_string(),
            axum::body::Bytes::new(),
        )
        .await
        .expect("forward should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let headers = captured_headers
            .lock()
            .expect("capture lock should not be poisoned");
        assert_eq!(headers.len(), 2);
        let expected_stale_header = format!("Bearer {stale_token}");
        let expected_fresh_header = format!("Bearer {fresh_token}");
        assert_eq!(
            headers[0]
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some(expected_stale_header.as_str())
        );
        assert_eq!(
            headers[1]
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some(expected_fresh_header.as_str())
        );
        server_handle.abort();
    }

    #[derive(Clone)]
    struct TestExternalAuth {
        fresh_token: String,
    }

    #[async_trait]
    impl ExternalAuth for TestExternalAuth {
        fn auth_mode(&self) -> codex_app_server_protocol::AuthMode {
            codex_app_server_protocol::AuthMode::Chatgpt
        }

        async fn refresh(
            &self,
            _context: ExternalAuthRefreshContext,
        ) -> io::Result<ExternalAuthTokens> {
            Ok(ExternalAuthTokens::chatgpt(
                self.fresh_token.clone(),
                "account_id",
                /*chatgpt_plan_type*/ None,
            ))
        }
    }

    fn fake_jwt(email: &str) -> String {
        let header = serde_json::json!({ "alg": "none", "typ": "JWT" });
        let payload = serde_json::json!({ "email": email });
        let header_b64 =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header should serialize"));
        let payload_b64 =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload should serialize"));
        let signature_b64 = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header_b64}.{payload_b64}.{signature_b64}")
    }

    async fn spawn_test_backend(
        handler: impl Fn(HeaderMap) -> StatusCode + Clone + Send + Sync + 'static,
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let router = Router::new().route(
            "/api/codex/example/auth-test",
            any(move |headers: HeaderMap| {
                let handler = handler.clone();
                async move { handler(headers) }
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("server should run");
        });
        (format!("http://{address}"), handle)
    }
}
