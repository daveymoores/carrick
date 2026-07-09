//! GitHub Actions OIDC token minting for keyless cloud auth.
//!
//! When the Action runs in GitHub Actions with `id-token: write` permission,
//! the runner exposes `ACTIONS_ID_TOKEN_REQUEST_URL` and
//! `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. We exchange those for a short-lived OIDC
//! JWT scoped to the `https://api.carrick.tools` audience and send it as the
//! `X-Carrick-OIDC` header on every cloud request. The cloud derives the repo
//! identity (owner, repo, repo id) from the signed claims, so no API key is
//! needed.
//!
//! Tokens are short-lived (~minutes). We mint once and cache for the run; on a
//! 401 mid-run (long scans can outlive a token) callers re-mint via
//! [`OidcProvider::remint`] and retry once. The cloud allows ~30s clock skew.

use std::env;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::warn;

/// Audience the cloud requires in the OIDC token's `aud` claim.
const AUDIENCE: &str = "https://api.carrick.tools";

/// Deadline for the token request — minting must be fast, and a hung GitHub
/// endpoint must not stall the scan indefinitely.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Retries after the first attempt for transient mint failures (transport
/// errors, 5xx from the GitHub token endpoint).
const MAX_FETCH_RETRIES: u32 = 2;

#[derive(Debug)]
pub enum OidcError {
    /// Not running with `id-token: write` (the request env vars are absent).
    Unavailable,
    /// The token request to GitHub failed at the transport layer.
    Request(String),
    /// GitHub returned a non-success status or an unparseable body.
    BadResponse(String),
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Unavailable => write!(
                f,
                "GitHub Actions OIDC is not available: ACTIONS_ID_TOKEN_REQUEST_URL / \
                 ACTIONS_ID_TOKEN_REQUEST_TOKEN are not set. Add `permissions: id-token: write` \
                 to the workflow job so Carrick can authenticate to the cloud without an API key. \
                 Note: GitHub never grants OIDC credentials to pull_request runs from forks — \
                 fork PRs cannot authenticate, and the Carrick Action skips them instead of \
                 failing."
            ),
            OidcError::Request(e) => write!(f, "OIDC token request failed: {}", e),
            OidcError::BadResponse(e) => write!(f, "OIDC token endpoint error: {}", e),
        }
    }
}

impl std::error::Error for OidcError {}

/// Process-wide OIDC token provider. The request URL/token and the minted JWT
/// are global to the run, so this is a singleton reached via [`OidcProvider::global`].
pub struct OidcProvider {
    client: reqwest::Client,
    request_url: String,
    request_token: String,
    cached: Mutex<Option<String>>,
}

static PROVIDER: OnceLock<Option<OidcProvider>> = OnceLock::new();

impl OidcProvider {
    /// Returns the shared provider, or [`OidcError::Unavailable`] if the runner
    /// did not expose the OIDC request env vars (i.e. the job lacks
    /// `id-token: write`).
    pub fn global() -> Result<&'static OidcProvider, OidcError> {
        PROVIDER
            .get_or_init(OidcProvider::from_env)
            .as_ref()
            .ok_or(OidcError::Unavailable)
    }

    fn from_env() -> Option<OidcProvider> {
        let request_url = env::var("ACTIONS_ID_TOKEN_REQUEST_URL").ok()?;
        let request_token = env::var("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok()?;
        Some(OidcProvider {
            client: reqwest::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .build()
                .expect("default reqwest client construction cannot fail"),
            request_url,
            request_token,
            cached: Mutex::new(None),
        })
    }

    /// Returns the cached token, minting it on first use.
    pub async fn token(&self) -> Result<String, OidcError> {
        let mut guard = self.cached.lock().await;
        if let Some(token) = guard.as_ref() {
            return Ok(token.clone());
        }
        let token = self.fetch().await?;
        *guard = Some(token.clone());
        Ok(token)
    }

    /// Forces a fresh mint, replacing the cache. Call after a 401 when the
    /// cached token may have expired mid-run.
    pub async fn remint(&self) -> Result<String, OidcError> {
        let mut guard = self.cached.lock().await;
        let token = self.fetch().await?;
        *guard = Some(token.clone());
        Ok(token)
    }

    async fn fetch(&self) -> Result<String, OidcError> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            value: String,
        }

        let mut retries = 0u32;
        loop {
            // `.query()` merges into the URL's existing query string (the
            // request URL already carries `?api-version=...`) and
            // percent-encodes the audience, matching the official
            // @actions/core toolkit behaviour.
            let transient_error = match self
                .client
                .get(&self.request_url)
                .query(&[("audience", AUDIENCE)])
                .header("Authorization", format!("Bearer {}", self.request_token))
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let parsed: TokenResponse = response.json().await.map_err(|e| {
                            OidcError::BadResponse(format!("failed to parse token response: {}", e))
                        })?;
                        return Ok(parsed.value);
                    }

                    let body = response.text().await.unwrap_or_default();
                    let err = OidcError::BadResponse(format!(
                        "GitHub token endpoint returned {}: {}",
                        status, body
                    ));
                    // 4xx means the request itself is bad (missing permission,
                    // bad token) — retrying can't fix it.
                    if !status.is_server_error() {
                        return Err(err);
                    }
                    err
                }
                Err(e) => OidcError::Request(e.to_string()),
            };

            if retries >= MAX_FETCH_RETRIES {
                return Err(transient_error);
            }

            let backoff = Duration::from_secs(1u64 << retries);
            warn!(
                "{}; retrying OIDC token mint in {}s ({}/{})",
                transient_error,
                backoff.as_secs(),
                retries + 1,
                MAX_FETCH_RETRIES
            );
            tokio::time::sleep(backoff).await;
            retries += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    impl OidcProvider {
        /// Test-only constructor pointing the provider at an arbitrary endpoint.
        fn for_test(request_url: String, request_token: String) -> Self {
            OidcProvider {
                // no_proxy so CI proxy env vars can't intercept the localhost call.
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
                request_url,
                request_token,
                cached: Mutex::new(None),
            }
        }
    }

    /// Verifies the exact contract the cloud expects: the audience is appended
    /// to the runner-provided request URL (preserving its existing query),
    /// percent-encoded to decode back to `https://api.carrick.tools`, the
    /// request token is forwarded as a bearer header, and `.value` is the JWT.
    #[tokio::test]
    async fn fetch_appends_audience_and_parses_value() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let body = r#"{"value":"header.payload.signature"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            request
        });

        let url = format!("http://{}/token?api-version=2.0", addr);
        let provider = OidcProvider::for_test(url, "request-token-xyz".to_string());

        let token = provider.fetch().await.unwrap();
        assert_eq!(token, "header.payload.signature");

        let request = server.join().unwrap();
        let request_line = request.lines().next().unwrap_or_default();
        // Existing query param preserved + audience appended and encoded so the
        // GitHub token service decodes it back to the exact required audience.
        assert!(
            request_line.contains("api-version=2.0"),
            "existing query dropped: {request_line}"
        );
        assert!(
            request_line.contains("audience=https%3A%2F%2Fapi.carrick.tools"),
            "audience missing/mis-encoded: {request_line}"
        );
        // Request token forwarded as bearer (header name case-insensitive).
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer request-token-xyz"),
            "bearer auth header missing: {request}"
        );
    }

    /// A transient 5xx from the token endpoint is retried; the mint succeeds
    /// on the second attempt instead of aborting the scan.
    #[tokio::test]
    async fn fetch_retries_transient_server_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let responses = [
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
                {
                    let body = r#"{"value":"retried.token.ok"}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                },
            ];
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });

        let url = format!("http://{}/token?api-version=2.0", addr);
        let provider = OidcProvider::for_test(url, "request-token-xyz".to_string());

        let token = provider.fetch().await.unwrap();
        assert_eq!(token, "retried.token.ok");
        server.join().unwrap();
    }

    /// A 4xx from the token endpoint (bad permission/token) is permanent —
    /// no retry, error returned immediately.
    #[tokio::test]
    async fn fetch_does_not_retry_client_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf).unwrap();
            let response =
                "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            // A second connection attempt would block here and fail the test
            // via join timeout if fetch retried; instead the listener is
            // dropped right after the first response.
        });

        let url = format!("http://{}/token?api-version=2.0", addr);
        let provider = OidcProvider::for_test(url, "request-token-xyz".to_string());

        let err = provider.fetch().await.unwrap_err();
        assert!(
            matches!(&err, OidcError::BadResponse(msg) if msg.contains("403")),
            "expected permanent 403 error, got: {err}"
        );
        server.join().unwrap();
    }
}
