//! Shared HTTP client built on top of `reqwest`.
//!
//! Wraps a [`reqwest::Client`] with sensible defaults (timeouts, TLS,
//! proxy support) and a small retry helper for transient transport failures.
//! Timeouts are **not** retried — the caller already decided the request had
//! enough time.

use std::time::Duration;

use reqwest::Client;
use tokio::time::sleep;
use tracing::debug;

use crate::error::{NetworkError, Result};

/// Default request timeout (matches the Kotlin `Innertube` 30 s ceiling).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Default connect timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A configured `reqwest` client with optional proxy and retry.
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: Client,
    /// Number of extra retries for transient (non-timeout) transport errors.
    retry_attempts: u32,
}

impl HttpClient {
    /// Build a client with default timeouts and no proxy.
    pub fn new() -> Result<Self> {
        let client = Self::builder(Default::default()).build()?;
        Ok(Self {
            client,
            retry_attempts: 3,
        })
    }

    /// Builder-style entry point that applies the standard timeout defaults.
    pub fn builder(proxy: Option<reqwest::Url>) -> reqwest::ClientBuilder {
        let mut b = Client::builder()
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .user_agent(crate::innertube::WEB_USER_AGENT);
        if let Some(url) = proxy {
            b = b.proxy(reqwest::Proxy::all(url));
        }
        b
    }

    /// Build a client that routes all traffic through `proxy_url`.
    pub fn with_proxy(proxy_url: &str) -> Result<Self> {
        let url: reqwest::Url = proxy_url
            .parse()
            .map_err(|e| NetworkError::Transport(reqwest::Error::new(reqwest::StatusCode::BAD_REQUEST, e)))?;
        let client = Self::builder(Some(url)).build()?;
        Ok(Self {
            client,
            retry_attempts: 3,
        })
    }

    /// Perform a GET request and return the raw response.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response> {
        self.request(reqwest::Method::GET, url, None, &[]).await
    }

    /// Perform a POST request with a JSON body.
    pub async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        query: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        self.request(reqwest::Method::POST, url, Some(body), query)
            .await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        json: Option<&serde_json::Value>,
        query: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        let mut req = self.client.request(method.clone(), url);
        for (k, v) in query {
            req = req.query(&[(k, v)]);
        }
        if let Some(body) = json {
            req = req.json(body);
        }
        Ok(req.send().await?)
    }

    /// Execute `f`, retrying on transient transport errors (connection reset,
    /// DNS blip, etc.). Timeouts are **not** retried: a timeout is the app's
    /// own ceiling, and repeating the request only multiplies the cost.
    pub async fn with_retry<T>(
        &self,
        f: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let mut backoff = Duration::from_millis(500);
        let attempts = self.retry_attempts;

        for attempt in 0..attempts.saturating_sub(1) {
            match f.try_await().await {
                Ok(v) => return Ok(v),
                Err(NetworkError::Timeout(msg)) => {
                    // Do not retry timeouts — the ceiling has already been hit.
                    debug!(
                        attempt = attempt + 1,
                        "not retrying, request timed out: {msg}"
                    );
                    return Err(NetworkError::Timeout(msg));
                }
                Err(e) => {
                    debug!(
                        attempt = attempt + 1,
                        error = %e,
                        "transient error, retrying"
                    );
                    sleep(backoff).await;
                    backoff *= 2;
                }
            }
        }
        // Final attempt.
        f.try_await().await
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("default reqwest client must build")
    }
}

/// Adapter that lets [`with_retry`](HttpClient::with_retry) use `try_await`
/// on plain `Future` values without pulling in `futures::TryFuture`.
trait TryFuture<T>: std::future::Future<Output = Result<T>> + Sized
where
    T: Sized,
{
    async fn try_await(self) -> Result<T> {
        self.await
    }
}

impl<F, T> TryFuture<T> for F
where
    F: std::future::Future<Output = Result<T>>,
{
}
