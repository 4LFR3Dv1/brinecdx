use std::future::Future;
use std::pin::Pin;

use http::HeaderMap;
use http::HeaderValue;
use http::header::AUTHORIZATION;

use crate::ExecServerError;
use crate::client_api::DEFAULT_REMOTE_EXEC_SERVER_CONNECT_TIMEOUT;
use crate::client_api::DEFAULT_REMOTE_EXEC_SERVER_INITIALIZE_TIMEOUT;
use crate::client_api::ExecServerTransportParams;
use crate::environment::CODEX_EXEC_SERVER_URL_ENV_VAR;
use crate::environment::LOCAL_ENVIRONMENT_ID;
use crate::environment::REMOTE_ENVIRONMENT_ID;

const BRINE_EXEC_SERVER_URL_ENV_VAR: &str = "BRINE_EXEC_SERVER_URL";
const BRINE_EXEC_SERVER_TOKEN_ENV_VAR: &str = "BRINE_EXEC_SERVER_TOKEN";

/// Lists the remote environment transports available to Codex.
///
/// Implementations own a startup snapshot containing both the available
/// environment transport list in configured order and the default environment
/// selection. Providers return transport descriptions before the effective HTTP
/// policy is available; `include_local` controls whether `EnvironmentManager`
/// should add the local environment when the snapshot is built.
pub trait EnvironmentProvider: Send + Sync {
    /// Returns the provider-owned environment startup snapshot.
    fn snapshot(&self) -> EnvironmentProviderFuture<'_>;
}

pub type EnvironmentProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<EnvironmentProviderSnapshot, ExecServerError>> + Send + 'a>>;

#[derive(Clone)]
pub struct EnvironmentProviderSnapshot {
    pub(crate) environments: Vec<(String, ExecServerTransportParams)>,
    pub default: EnvironmentDefault,
    pub include_local: bool,
}

impl std::fmt::Debug for EnvironmentProviderSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let environment_ids: Vec<_> = self.environments.iter().map(|(id, _)| id).collect();
        f.debug_struct("EnvironmentProviderSnapshot")
            .field("environments", &environment_ids)
            .field("default", &self.default)
            .field("include_local", &self.include_local)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentDefault {
    Disabled,
    EnvironmentId(String),
}

/// Default provider backed by a Brine exec-server when configured, otherwise
/// by Codex's standard `CODEX_EXEC_SERVER_URL` setting.
#[derive(Clone)]
pub struct DefaultEnvironmentProvider {
    exec_server_url: Option<String>,
    http_headers: HeaderMap,
}

impl std::fmt::Debug for DefaultEnvironmentProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultEnvironmentProvider")
            .field("exec_server_url", &self.exec_server_url)
            .field("http_headers", &"<redacted>")
            .finish()
    }
}

impl DefaultEnvironmentProvider {
    /// Builds a provider from an already-read raw exec-server URL value.
    pub fn new(exec_server_url: Option<String>) -> Self {
        Self {
            exec_server_url,
            http_headers: HeaderMap::new(),
        }
    }

    /// Builds a provider by reading the BrineCDX override first and then the
    /// upstream Codex setting. This keeps upstream behavior unchanged unless
    /// `BRINE_EXEC_SERVER_URL` is explicitly present.
    pub fn from_env() -> Self {
        Self::from_env_values(
            std::env::var(BRINE_EXEC_SERVER_URL_ENV_VAR).ok(),
            std::env::var(BRINE_EXEC_SERVER_TOKEN_ENV_VAR).ok(),
            std::env::var(CODEX_EXEC_SERVER_URL_ENV_VAR).ok(),
        )
    }

    fn from_env_values(
        brine_exec_server_url: Option<String>,
        brine_exec_server_token: Option<String>,
        codex_exec_server_url: Option<String>,
    ) -> Self {
        match brine_exec_server_url {
            Some(url) => Self {
                exec_server_url: Some(url),
                http_headers: brine_authorization_headers(brine_exec_server_token),
            },
            None => Self::new(codex_exec_server_url),
        }
    }

    pub(crate) fn snapshot_inner(&self) -> EnvironmentProviderSnapshot {
        let mut environments = Vec::new();
        let (exec_server_url, disabled) = normalize_exec_server_url(self.exec_server_url.clone());

        if let Some(exec_server_url) = exec_server_url {
            environments.push((
                REMOTE_ENVIRONMENT_ID.to_string(),
                ExecServerTransportParams::WebSocketUrl {
                    websocket_url: exec_server_url,
                    connect_timeout: DEFAULT_REMOTE_EXEC_SERVER_CONNECT_TIMEOUT,
                    initialize_timeout: DEFAULT_REMOTE_EXEC_SERVER_INITIALIZE_TIMEOUT,
                    http_headers: self.http_headers.clone(),
                },
            ));
        }

        let has_remote = environments
            .iter()
            .any(|(id, _environment)| id == REMOTE_ENVIRONMENT_ID);
        let include_local = !disabled && !has_remote;
        let default = if disabled {
            EnvironmentDefault::Disabled
        } else if has_remote {
            EnvironmentDefault::EnvironmentId(REMOTE_ENVIRONMENT_ID.to_string())
        } else {
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        };

        EnvironmentProviderSnapshot {
            environments,
            default,
            include_local,
        }
    }
}

impl EnvironmentProvider for DefaultEnvironmentProvider {
    fn snapshot(&self) -> EnvironmentProviderFuture<'_> {
        Box::pin(async { Ok(self.snapshot_inner()) })
    }
}

fn brine_authorization_headers(token: Option<String>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let Some(token) = token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return headers;
    };
    // Invalid header bytes fail closed by omitting the credential. The Brine
    // server then rejects the WebSocket upgrade instead of accepting a malformed
    // or partially normalized secret.
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        headers.insert(AUTHORIZATION, value);
    }
    headers
}

pub(crate) fn normalize_exec_server_url(exec_server_url: Option<String>) -> (Option<String>, bool) {
    match exec_server_url.as_deref().map(str::trim) {
        None | Some("") => (None, false),
        Some(url) if url.eq_ignore_ascii_case("none") => (None, true),
        Some(url) => (Some(url.to_string()), false),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn brine_exec_server_url_takes_precedence_over_codex_url() {
        let provider = DefaultEnvironmentProvider::from_env_values(
            Some("ws://127.0.0.1:8766".to_string()),
            None,
            Some("ws://127.0.0.1:8765".to_string()),
        );
        let (url, disabled) = normalize_exec_server_url(provider.exec_server_url);

        assert_eq!(url.as_deref(), Some("ws://127.0.0.1:8766"));
        assert!(!disabled);
    }

    #[test]
    fn codex_exec_server_url_remains_the_fallback() {
        let provider = DefaultEnvironmentProvider::from_env_values(
            None,
            Some("must-not-leak-to-upstream".to_string()),
            Some("ws://127.0.0.1:8765".to_string()),
        );
        let (url, disabled) = normalize_exec_server_url(provider.exec_server_url.clone());

        assert_eq!(url.as_deref(), Some("ws://127.0.0.1:8765"));
        assert!(!disabled);
        assert!(provider.http_headers.is_empty());
    }

    #[test]
    fn brine_token_becomes_only_a_redacted_websocket_authorization_header() {
        let provider = DefaultEnvironmentProvider::from_env_values(
            Some("wss://brine.example/exec".to_string()),
            Some("secret-token-0123456789".to_string()),
            None,
        );
        let snapshot = provider.snapshot_inner();
        let (_, transport) = snapshot
            .environments
            .iter()
            .find(|(id, _)| id == REMOTE_ENVIRONMENT_ID)
            .expect("remote environment");
        let ExecServerTransportParams::WebSocketUrl { http_headers, .. } = transport else {
            panic!("expected websocket transport");
        };
        assert_eq!(
            http_headers
                .get(AUTHORIZATION)
                .expect("authorization")
                .to_str()
                .expect("header text"),
            "Bearer secret-token-0123456789"
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-token-0123456789"));
    }

    #[tokio::test]
    async fn default_provider_requests_local_environment_when_url_is_missing() {
        let provider = DefaultEnvironmentProvider::new(/*exec_server_url*/ None);
        let snapshot = provider.snapshot().await.expect("environments");
        let EnvironmentProviderSnapshot {
            environments,
            default,
            include_local,
        } = snapshot;
        let environments: HashMap<_, _> = environments.into_iter().collect();

        assert!(include_local);
        assert!(!environments.contains_key(LOCAL_ENVIRONMENT_ID));
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(
            default,
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_requests_local_environment_when_url_is_empty() {
        let provider = DefaultEnvironmentProvider::new(Some(String::new()));
        let snapshot = provider.snapshot().await.expect("environments");
        let EnvironmentProviderSnapshot {
            environments,
            default,
            include_local,
        } = snapshot;
        let environments: HashMap<_, _> = environments.into_iter().collect();

        assert!(include_local);
        assert!(!environments.contains_key(LOCAL_ENVIRONMENT_ID));
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(
            default,
            EnvironmentDefault::EnvironmentId(LOCAL_ENVIRONMENT_ID.to_string())
        );
    }

    #[tokio::test]
    async fn default_provider_omits_local_environment_for_none_value() {
        let provider = DefaultEnvironmentProvider::new(Some("none".to_string()));
        let snapshot = provider.snapshot().await.expect("environments");
        let EnvironmentProviderSnapshot {
            environments,
            default,
            include_local,
        } = snapshot;
        let environments: HashMap<_, _> = environments.into_iter().collect();

        assert!(!include_local);
        assert!(!environments.contains_key(LOCAL_ENVIRONMENT_ID));
        assert!(!environments.contains_key(REMOTE_ENVIRONMENT_ID));
        assert_eq!(default, EnvironmentDefault::Disabled);
    }

    #[tokio::test]
    async fn default_provider_adds_remote_environment_for_websocket_url() {
        let provider = DefaultEnvironmentProvider::new(Some("ws://127.0.0.1:8765".to_string()));
        let snapshot = provider.snapshot().await.expect("environments");
        let EnvironmentProviderSnapshot {
            environments,
            default,
            include_local,
        } = snapshot;
        let environments: HashMap<_, _> = environments.into_iter().collect();

        assert!(!include_local);
        assert!(!environments.contains_key(LOCAL_ENVIRONMENT_ID));
        assert!(matches!(
            &environments[REMOTE_ENVIRONMENT_ID],
            ExecServerTransportParams::WebSocketUrl { websocket_url, .. }
                if websocket_url == "ws://127.0.0.1:8765"
        ));
        assert_eq!(
            default,
            EnvironmentDefault::EnvironmentId(REMOTE_ENVIRONMENT_ID.to_string())
        );
    }

    #[test]
    fn normalizes_exec_server_url() {
        assert_eq!(
            normalize_exec_server_url(Some(" ws://127.0.0.1:8765 ".to_string())),
            (Some("ws://127.0.0.1:8765".to_string()), false)
        );
    }
}
