use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use codex_exec_server::HttpClient;
use codex_exec_server::HttpHeader;
use codex_exec_server::HttpRequestParams;
use oauth2::AuthUrl;
use oauth2::AuthorizationCode;
use oauth2::ClientId;
use oauth2::ClientSecret;
use oauth2::CsrfToken;
use oauth2::EmptyExtraTokenFields;
use oauth2::PkceCodeChallenge;
use oauth2::PkceCodeVerifier;
use oauth2::RedirectUrl;
use oauth2::RefreshToken;
use oauth2::RequestTokenError;
use oauth2::Scope;
use oauth2::StandardErrorResponse;
use oauth2::StandardTokenResponse;
use oauth2::TokenResponse;
use oauth2::TokenUrl;
use oauth2::basic::BasicClient;
use oauth2::basic::BasicErrorResponseType;
use oauth2::basic::BasicTokenType;
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde::Serialize;
use tracing::debug;

use crate::WrappedOAuthTokenResponse;
use crate::oauth::StoredOAuthTokens;
use crate::oauth::compute_expires_at_millis;
use crate::utils::build_default_headers;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const OAUTH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const OAUTH_DISCOVERY_HEADER: &str = "MCP-Protocol-Version";
const OAUTH_DISCOVERY_VERSION: &str = "2024-11-05";

type OAuthTokenResponse = StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>;
type OAuthErrorResponse = StandardErrorResponse<BasicErrorResponseType>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamableHttpOAuthDiscovery {
    pub scopes_supported: Option<Vec<String>>,
}

#[derive(Clone)]
pub(crate) struct OAuthHttpClient {
    http_client: Arc<dyn HttpClient>,
    default_headers: HeaderMap,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct OAuthHttpError(String);

#[derive(Debug)]
pub(crate) struct OAuthAuthorizationSession {
    pub authorization_url: String,
    pub csrf_state: CsrfToken,
    pkce_verifier: PkceCodeVerifier,
    client: BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
    client_id: String,
}

#[derive(Debug, Clone)]
struct OAuthClientConfig {
    client_id: String,
    client_secret: Option<String>,
}

impl OAuthHttpClient {
    pub(crate) fn new(
        http_client: Arc<dyn HttpClient>,
        http_headers: Option<HashMap<String, String>>,
        env_http_headers: Option<HashMap<String, String>>,
    ) -> Result<Self> {
        let default_headers = build_default_headers(http_headers, env_http_headers)?;
        Ok(Self {
            http_client,
            default_headers,
        })
    }

    pub(crate) fn from_default_headers(
        http_client: Arc<dyn HttpClient>,
        default_headers: HeaderMap,
    ) -> Self {
        Self {
            http_client,
            default_headers,
        }
    }

    pub(crate) async fn discover(&self, url: &str) -> Result<Option<StreamableHttpOAuthDiscovery>> {
        let metadata = match self.discover_metadata(url).await? {
            Some(metadata) => metadata,
            None => return Ok(None),
        };

        Ok(Some(StreamableHttpOAuthDiscovery {
            scopes_supported: normalize_scopes(metadata.scopes_supported),
        }))
    }

    async fn discover_metadata(&self, url: &str) -> Result<Option<OAuthDiscoveryMetadata>> {
        let base_url = Url::parse(url)?;
        let mut last_error: Option<anyhow::Error> = None;

        for candidate_path in discovery_paths(base_url.path()) {
            let mut discovery_url = base_url.clone();
            discovery_url.set_path(&candidate_path);

            let response = match self
                .request(
                    "GET",
                    discovery_url.as_str(),
                    vec![HttpHeader {
                        name: OAUTH_DISCOVERY_HEADER.to_string(),
                        value: OAUTH_DISCOVERY_VERSION.to_string(),
                    }],
                    /*body*/ None,
                    Some(DISCOVERY_TIMEOUT),
                )
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(err);
                    continue;
                }
            };

            if response.status != StatusCode::OK.as_u16() {
                continue;
            }

            let metadata = match serde_json::from_slice::<OAuthDiscoveryMetadata>(&response.body.0)
            {
                Ok(metadata) => metadata,
                Err(err) => {
                    last_error = Some(err.into());
                    continue;
                }
            };

            if metadata.authorization_endpoint.is_some() && metadata.token_endpoint.is_some() {
                return Ok(Some(metadata));
            }
        }

        if let Some(err) = last_error {
            debug!("OAuth discovery requests failed for {url}: {err:?}");
        }

        Ok(None)
    }

    pub(crate) async fn start_authorization(
        &self,
        server_url: &str,
        scopes: &[String],
        redirect_uri: &str,
        client_name: &str,
    ) -> Result<OAuthAuthorizationSession> {
        let metadata = self
            .discover_metadata(server_url)
            .await?
            .ok_or_else(|| anyhow!("No authorization support detected"))?;
        let client_config = self
            .register_client(&metadata, client_name, redirect_uri)
            .await?;
        let client = oauth_client(&metadata, &client_config, redirect_uri)?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut auth_request = client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge);
        for scope in scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.clone()));
        }
        let (authorization_url, csrf_state) = auth_request.url();

        Ok(OAuthAuthorizationSession {
            authorization_url: authorization_url.to_string(),
            csrf_state,
            pkce_verifier,
            client,
            client_id: client_config.client_id,
        })
    }

    pub(crate) async fn exchange_code(
        &self,
        session: OAuthAuthorizationSession,
        code: &str,
        csrf_state: &str,
    ) -> Result<(String, OAuthTokenResponse)> {
        if session.csrf_state.secret() != csrf_state {
            return Err(anyhow!(
                "OAuth callback state did not match authorization request"
            ));
        }

        let http_client = self.clone();
        let token = session
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(session.pkce_verifier)
            .request_async(&|request| {
                let http_client = http_client.clone();
                async move { http_client.oauth_request(request).await }
            })
            .await
            .or_else(parse_token_from_parse_error)?;

        Ok((session.client_id, token))
    }

    pub(crate) async fn refresh_token(
        &self,
        tokens: &StoredOAuthTokens,
    ) -> Result<Option<StoredOAuthTokens>> {
        let refresh_token = match tokens.token_response.0.refresh_token() {
            Some(refresh_token) => refresh_token.secret().to_string(),
            None => return Ok(None),
        };

        let metadata = self
            .discover_metadata(&tokens.url)
            .await?
            .ok_or_else(|| anyhow!("No authorization support detected"))?;
        let client_config = OAuthClientConfig {
            client_id: tokens.client_id.clone(),
            client_secret: None,
        };
        let client = oauth_client(&metadata, &client_config, &tokens.url)?;
        let http_client = self.clone();
        let token = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .request_async(&|request| {
                let http_client = http_client.clone();
                async move { http_client.oauth_request(request).await }
            })
            .await
            .or_else(parse_token_from_parse_error)?;

        let expires_at = compute_expires_at_millis(&token);
        Ok(Some(StoredOAuthTokens {
            server_name: tokens.server_name.clone(),
            url: tokens.url.clone(),
            client_id: tokens.client_id.clone(),
            token_response: WrappedOAuthTokenResponse(token),
            expires_at,
        }))
    }

    async fn register_client(
        &self,
        metadata: &OAuthDiscoveryMetadata,
        client_name: &str,
        redirect_uri: &str,
    ) -> Result<OAuthClientConfig> {
        let registration_url = metadata
            .registration_endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("Dynamic client registration not supported"))?;

        if let Some(response_types_supported) = metadata.response_types_supported.as_ref()
            && !response_types_supported.iter().any(|value| value == "code")
        {
            return Err(anyhow!(
                "OAuth server does not support authorization code flow"
            ));
        }

        let body = serde_json::to_vec(&ClientRegistrationRequest {
            client_name: client_name.to_string(),
            redirect_uris: vec![redirect_uri.to_string()],
            grant_types: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            token_endpoint_auth_method: "none".to_string(),
            response_types: vec!["code".to_string()],
        })?;

        let response = self
            .request(
                "POST",
                registration_url,
                vec![HttpHeader {
                    name: reqwest::header::CONTENT_TYPE.to_string(),
                    value: "application/json".to_string(),
                }],
                Some(body),
                Some(OAUTH_HTTP_TIMEOUT),
            )
            .await?;

        if !status_is_success(response.status) {
            return Err(anyhow!(
                "Dynamic registration failed: HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body.0)
            ));
        }

        let registration: ClientRegistrationResponse = serde_json::from_slice(&response.body.0)
            .context("failed to parse registration response")?;
        Ok(OAuthClientConfig {
            client_id: registration.client_id,
            client_secret: registration
                .client_secret
                .filter(|secret| !secret.is_empty()),
        })
    }

    async fn oauth_request(
        &self,
        request: oauth2::HttpRequest,
    ) -> std::result::Result<oauth2::HttpResponse, OAuthHttpError> {
        let (parts, body) = request.into_parts();
        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| {
                let value = value
                    .to_str()
                    .map_err(|err| OAuthHttpError(format!("invalid OAuth header value: {err}")))?;
                Ok(HttpHeader {
                    name: name.to_string(),
                    value: value.to_string(),
                })
            })
            .collect::<std::result::Result<Vec<_>, OAuthHttpError>>()?;
        let response = self
            .request(
                parts.method.as_str(),
                parts.uri.to_string().as_str(),
                headers,
                Some(body),
                Some(OAUTH_HTTP_TIMEOUT),
            )
            .await
            .map_err(|err| OAuthHttpError(err.to_string()))?;

        let mut oauth_response = oauth2::HttpResponse::new(response.body.0);
        *oauth_response.status_mut() = oauth2::http::StatusCode::from_u16(response.status)
            .map_err(|err| OAuthHttpError(format!("invalid OAuth response status: {err}")))?;
        for header in response.headers {
            let name = oauth2::http::HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|err| OAuthHttpError(format!("invalid OAuth response header: {err}")))?;
            let value = oauth2::http::HeaderValue::from_str(&header.value).map_err(|err| {
                OAuthHttpError(format!("invalid OAuth response header value: {err}"))
            })?;
            oauth_response.headers_mut().append(name, value);
        }
        Ok(oauth_response)
    }

    async fn request(
        &self,
        method: &str,
        url: &str,
        extra_headers: Vec<HttpHeader>,
        body: Option<Vec<u8>>,
        timeout: Option<Duration>,
    ) -> Result<codex_exec_server::HttpRequestResponse> {
        let mut headers = protocol_headers(&self.default_headers)?;
        headers.extend(extra_headers);
        self.http_client
            .http_request(HttpRequestParams {
                method: method.to_string(),
                url: url.to_string(),
                headers,
                body: body.map(Into::into),
                timeout_ms: timeout
                    .map(|timeout| timeout.as_millis().clamp(1, u64::MAX as u128) as u64),
                request_id: "oauth-request".to_string(),
                stream_response: false,
            })
            .await
            .map_err(|err| anyhow!(err))
    }
}

#[derive(Debug, Deserialize)]
struct OAuthDiscoveryMetadata {
    #[serde(default)]
    authorization_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    scopes_supported: Option<Vec<String>>,
    #[serde(default)]
    response_types_supported: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct ClientRegistrationRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    token_endpoint_auth_method: String,
    response_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ClientRegistrationResponse {
    client_id: String,
    client_secret: Option<String>,
}

fn oauth_client(
    metadata: &OAuthDiscoveryMetadata,
    config: &OAuthClientConfig,
    redirect_uri: &str,
) -> Result<
    BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    let authorization_endpoint = metadata
        .authorization_endpoint
        .clone()
        .ok_or_else(|| anyhow!("OAuth metadata did not include authorization endpoint"))?;
    let token_endpoint = metadata
        .token_endpoint
        .clone()
        .ok_or_else(|| anyhow!("OAuth metadata did not include token endpoint"))?;
    let mut client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(AuthUrl::new(authorization_endpoint)?)
        .set_token_uri(TokenUrl::new(token_endpoint)?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?);
    if let Some(secret) = config.client_secret.clone() {
        client = client.set_client_secret(ClientSecret::new(secret));
    }
    Ok(client)
}

fn parse_token_from_parse_error(
    error: RequestTokenError<OAuthHttpError, OAuthErrorResponse>,
) -> std::result::Result<OAuthTokenResponse, RequestTokenError<OAuthHttpError, OAuthErrorResponse>>
{
    match error {
        RequestTokenError::Parse(parse_error, body) => {
            match serde_json::from_slice::<OAuthTokenResponse>(&body) {
                Ok(parsed) => Ok(parsed),
                Err(_) => Err(RequestTokenError::Parse(parse_error, body)),
            }
        }
        error => Err(error),
    }
}

fn protocol_headers(headers: &HeaderMap) -> Result<Vec<HttpHeader>> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = value
                .to_str()
                .with_context(|| format!("invalid HTTP header value for `{name}`"))?;
            Ok(HttpHeader {
                name: name.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

fn status_is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

pub(crate) fn normalize_scopes(scopes_supported: Option<Vec<String>>) -> Option<Vec<String>> {
    let scopes_supported = scopes_supported?;

    let mut normalized = Vec::new();
    for scope in scopes_supported {
        let scope = scope.trim();
        if scope.is_empty() {
            continue;
        }
        let scope = scope.to_string();
        if !normalized.contains(&scope) {
            normalized.push(scope);
        }
    }

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Implements RFC 8414 section 3.1 for discovering well-known oauth endpoints.
pub(crate) fn discovery_paths(base_path: &str) -> Vec<String> {
    let trimmed = base_path.trim_start_matches('/').trim_end_matches('/');
    let canonical = "/.well-known/oauth-authorization-server".to_string();

    if trimmed.is_empty() {
        return vec![canonical];
    }

    let mut candidates = Vec::new();
    let mut push_unique = |candidate: String| {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    };

    push_unique(format!("{canonical}/{trimmed}"));
    push_unique(format!("/{trimmed}/.well-known/oauth-authorization-server"));
    push_unique(canonical);

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn discovery_paths_prefer_rfc_8414_resource_path() {
        assert_eq!(
            discovery_paths("/mcp"),
            vec![
                "/.well-known/oauth-authorization-server/mcp".to_string(),
                "/mcp/.well-known/oauth-authorization-server".to_string(),
                "/.well-known/oauth-authorization-server".to_string(),
            ]
        );
    }

    #[test]
    fn discovery_paths_deduplicate_root_path() {
        assert_eq!(
            discovery_paths("/"),
            vec!["/.well-known/oauth-authorization-server".to_string()]
        );
    }

    #[test]
    fn normalize_scopes_trims_empties_and_deduplicates() {
        assert_eq!(
            normalize_scopes(Some(vec![
                "read".to_string(),
                " write ".to_string(),
                "".to_string(),
                "read".to_string(),
            ])),
            Some(vec!["read".to_string(), "write".to_string()])
        );
    }
}
