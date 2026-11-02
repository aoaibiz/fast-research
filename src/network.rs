//! HTTP layer, ported from `searx/network/`.
//!
//! Responsibilities kept from upstream: per request timeout, proxies, a
//! redirect limit, retries on transport errors and a response cache.
//!
//! Identity: by default (`outgoing.identity = "browser"`) requests carry the
//! upstream rotating desktop Firefox User-Agent. With `outgoing.identity =
//! "honest"` every engine and page request carries one fixed User-Agent that
//! names this tool, also over an engine's own User-Agent header.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use tracing::debug;

use crate::cache::{Cache, CachedResponse};
use crate::engine::{EngineRequest, EngineResponse, FailureKind, Method};
use crate::settings::Settings;

const USER_AGENT_OS: &[&str] = &[
    "Windows NT 10.0; Win64; x64",
    "X11; Linux x86_64",
    "Macintosh; Intel Mac OS X 10.15",
    "X11; Ubuntu; Linux x86_64",
];
const USER_AGENT_VERSIONS: &[&str] = &["157.0", "156.0", "155.0", "154.0"];
const USER_AGENT_TEMPLATE: &str =
    "Mozilla/5.0 ({os}; rv:{version}) Gecko/20100101 Firefox/{version}";

/// The default honest User-Agent (`outgoing.useragent`).
pub const HONEST_USER_AGENT: &str =
    "TalmudResearch/0.26 (+https://github.com/aoaibiz/fast-research)";

/// The honest User-Agent of the running process, set once from the settings.
/// `None` (or unset) means browser mode. Used by the few engines that build
/// their own client or header from [`default_user_agent`].
static PROCESS_HONEST_AGENT: OnceLock<Option<String>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("request timed out")]
    Timeout,
    #[error("http status {status}")]
    Status {
        status: u16,
        body: String,
        response: Box<EngineResponse>,
    },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("public address required")]
    Blocked,
    #[error("response too large")]
    TooLarge,
    #[error("request could not be built: {0}")]
    Build(String),
}

impl NetworkError {
    /// Map a network error onto the framework failure kinds.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Timeout => FailureKind::Timeout,
            Self::Build(_) | Self::Blocked | Self::TooLarge => FailureKind::Other,
            Self::Transport(_) => FailureKind::Other,
            Self::Status { status, body, .. } => classify_status(*status, body),
        }
    }
}

/// Classify an HTTP status (with a body excerpt) the way upstream does.
pub fn classify_status(status: u16, body: &str) -> FailureKind {
    let lowered = body.to_lowercase();
    let captcha = lowered.contains("captcha")
        || lowered.contains("anomaly")
        || lowered.contains("unusual traffic")
        || lowered.contains("are you a robot");
    match status {
        403 if captcha => FailureKind::Captcha,
        401..=403 => FailureKind::AccessDenied,
        429 => FailureKind::TooManyRequests,
        500..=599 => FailureKind::Http,
        _ => FailureKind::Http,
    }
}

/// The HTTP client used by every engine.
pub struct Network {
    public_client: reqwest::Client,
    outgoing: crate::settings::Outgoing,
    user_agents: Vec<String>,
    suffix: String,
    /// `Some(agent)` in honest mode: the one User-Agent every request carries.
    honest_agent: Option<String>,
    retries: u32,
    counter: AtomicUsize,
}

/// The honest User-Agent for these settings, or `None` in browser mode.
pub fn honest_agent_for(outgoing: &crate::settings::Outgoing) -> Option<String> {
    if outgoing.identity == "browser" {
        return None;
    }
    let base = if outgoing.useragent.trim().is_empty() {
        HONEST_USER_AGENT.to_string()
    } else {
        outgoing.useragent.trim().to_string()
    };
    Some(
        format!("{} {}", base, outgoing.useragent_suffix)
            .trim()
            .to_string(),
    )
}

impl Network {
    pub fn from_settings(settings: &Settings) -> Result<Self, NetworkError> {
        let honest_agent = honest_agent_for(&settings.outgoing);
        let _ = PROCESS_HONEST_AGENT.set(honest_agent.clone());
        Ok(Self {
            public_client: crate::public_web::client()
                .map_err(|_| NetworkError::Build("public_client".into()))?,
            outgoing: settings.outgoing.clone(),
            user_agents: build_user_agents(),
            suffix: settings.outgoing.useragent_suffix.clone(),
            honest_agent,
            retries: settings.outgoing.retries,
            counter: AtomicUsize::new(0),
        })
    }

    /// Honest mode: the one fixed agent. Browser mode: a different user agent
    /// on every call, like upstream's rotation.
    pub fn user_agent(&self) -> String {
        if let Some(agent) = &self.honest_agent {
            return agent.clone();
        }
        if self.user_agents.is_empty() {
            return default_user_agent();
        }
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % self.user_agents.len();
        format!("{} {}", self.user_agents[index], self.suffix)
            .trim()
            .to_string()
    }

    fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.7"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("dnt"),
            HeaderValue::from_static("1"),
        );
        if let Ok(value) = HeaderValue::from_str(&self.user_agent()) {
            headers.insert(reqwest::header::USER_AGENT, value);
        }
        headers
    }

    /// Fetch a request, using the cache when one is given.
    pub async fn fetch(
        &self,
        request: EngineRequest,
        timeout: Duration,
        cache: Option<&Cache>,
        proxy: Option<&str>,
    ) -> Result<EngineResponse, NetworkError> {
        let parsed = url::Url::parse(&request.url).map_err(|_| NetworkError::Blocked)?;
        // This edition has no network exception for configured engine base URLs.
        // Private records live behind the independently authenticated workbench.
        if !crate::public_web::public_url(&parsed) {
            return Err(NetworkError::Blocked);
        }
        if proxy.is_some() || !self.outgoing.proxies.is_empty() {
            return Err(NetworkError::Build("public_proxy_not_supported".into()));
        }
        let client = self.public_client.clone();
        let body_repr = if let Some(json) = &request.json {
            json.to_string()
        } else {
            request
                .form
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&")
        };
        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let cache_key = Cache::key(method, &request.url, &body_repr);
        if let Some(cache) = cache {
            if let Some(cached) = cache.get(&cache_key).await {
                if cached.body.len() > crate::public_web::MAX_BODY_BYTES {
                    return Err(NetworkError::TooLarge);
                }
                debug!("cache hit");
                return Ok(EngineResponse {
                    status: cached.status,
                    final_url: cached.final_url,
                    headers: cached.headers,
                    body: cached.body.as_ref().clone(),
                });
            }
        }

        let deadline = tokio::time::Instant::now() + timeout;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(NetworkError::Timeout);
            }
            let outcome = self.send(client.clone(), &request, remaining).await;
            match outcome {
                Ok(response) => {
                    if let Some(cache) = cache {
                        if response.status == 200 && !response.body.is_empty() {
                            cache
                                .set(
                                    &cache_key,
                                    CachedResponse {
                                        status: response.status,
                                        final_url: response.final_url.clone(),
                                        headers: response.headers.clone(),
                                        body: std::sync::Arc::new(response.body.clone()),
                                    },
                                )
                                .await;
                        }
                    }
                    // A 429 usually clears within a second. Retrying once keeps
                    // a good engine (Yahoo! JAPAN, Bing) alive instead of
                    // suspending it for minutes.
                    if response.status == 429 && attempt <= self.retries {
                        let backoff = Duration::from_millis(700);
                        if tokio::time::Instant::now() + backoff < deadline {
                            debug!("rate limited, retrying after a pause");
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }
                    if response.status >= 400 {
                        return Err(NetworkError::Status {
                            status: response.status,
                            body: String::from_utf8_lossy(
                                &response.body[..response.body.len().min(4096)],
                            )
                            .to_string(),
                            response: Box::new(response),
                        });
                    }
                    return Ok(response);
                }
                Err(error) if attempt <= self.retries && is_retryable(&error) => {
                    // never retry a timeout: the engine deadline already elapsed
                    if matches!(error, NetworkError::Timeout) {
                        return Err(error);
                    }
                    let backoff = Duration::from_millis(250 * attempt as u64);
                    if tokio::time::Instant::now() + backoff >= deadline {
                        return Err(error);
                    }
                    debug!(attempt, "retrying transport request");
                    tokio::time::sleep(backoff).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Untrusted result URLs never inherit proxy, cookies, credentials or Referer.
    pub async fn fetch_public(
        &self,
        url: &str,
        timeout: Duration,
        cache: Option<&Cache>,
    ) -> Result<EngineResponse, NetworkError> {
        let parsed = url::Url::parse(url).map_err(|_| NetworkError::Blocked)?;
        if !crate::public_web::public_url(&parsed) {
            return Err(NetworkError::Blocked);
        }
        let key = Cache::key("PUBLIC_GET_V1", parsed.as_str(), "");
        if let Some(cache) = cache {
            if let Some(value) = cache.get(&key).await {
                if value.body.len() > crate::public_web::MAX_BODY_BYTES {
                    return Err(NetworkError::TooLarge);
                }
                let mut headers = value.headers;
                headers.insert("x-rustxng-cache".into(), "hit".into());
                return Ok(EngineResponse {
                    status: value.status,
                    final_url: value.final_url,
                    headers,
                    body: value.body.as_ref().clone(),
                });
            }
        }
        let request = EngineRequest::get(parsed.as_str());
        let mut result = self
            .send(self.public_client.clone(), &request, timeout)
            .await?;
        if result.status >= 400 {
            return Err(NetworkError::Status {
                status: result.status,
                body: String::new(),
                response: Box::new(result),
            });
        }
        result
            .headers
            .insert("x-rustxng-cache".into(), "miss".into());
        if result.status == 200 {
            if let Some(cache) = cache {
                cache
                    .set(
                        &key,
                        CachedResponse {
                            status: result.status,
                            final_url: result.final_url.clone(),
                            headers: result.headers.clone(),
                            body: Arc::new(result.body.clone()),
                        },
                    )
                    .await;
            }
        }
        Ok(result)
    }

    async fn send(
        &self,
        client: reqwest::Client,
        request: &EngineRequest,
        timeout: Duration,
    ) -> Result<EngineResponse, NetworkError> {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut headers = self.default_headers();
        for (name, value) in request.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
        // Honest mode: an engine's own browser-like User-Agent does not replace ours.
        if let Some(agent) = &self.honest_agent {
            if let Ok(value) = HeaderValue::from_str(agent) {
                headers.insert(reqwest::header::USER_AGENT, value);
            }
        }
        if !request.cookies.is_empty() {
            let cookie = request
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                headers.insert(reqwest::header::COOKIE, value);
            }
        }

        let mut builder = client.request(method, &request.url).headers(headers);
        if let Some(json) = &request.json {
            builder = builder.json(json);
        } else if request.method == Method::Post {
            builder = builder.form(&request.form);
        }

        let send = async {
            let mut response = builder.send().await.map_err(|error| {
                let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
                let mut blocked = false;
                while let Some(value) = cause {
                    let message = value.to_string();
                    if message.contains("public_dns_required")
                        || message.contains("public_redirect_required")
                    {
                        blocked = true;
                    }
                    cause = value.source();
                }
                if blocked {
                    NetworkError::Blocked
                } else if error.is_timeout() {
                    NetworkError::Timeout
                } else {
                    NetworkError::Transport(
                        if error.is_connect() {
                            "connect_failed"
                        } else {
                            "request_failed"
                        }
                        .into(),
                    )
                }
            })?;
            let status = response.status().as_u16();
            let final_url = response.url().to_string();
            let mut headers = BTreeMap::new();
            for (name, value) in response.headers().iter() {
                if let Ok(value) = value.to_str() {
                    headers.insert(name.as_str().to_lowercase(), value.to_string());
                }
            }
            if response
                .content_length()
                .is_some_and(|n| n > crate::public_web::MAX_BODY_BYTES as u64)
            {
                return Err(NetworkError::TooLarge);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| NetworkError::Transport("body_read_failed".into()))?
            {
                if chunk.len() > crate::public_web::MAX_BODY_BYTES.saturating_sub(body.len()) {
                    return Err(NetworkError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            headers.insert(
                "x-rustxng-fetched-at".into(),
                chrono::Utc::now().to_rfc3339(),
            );
            Ok(EngineResponse {
                status,
                final_url,
                headers,
                body,
            })
        };

        match tokio::time::timeout(timeout, send).await {
            Ok(result) => result,
            Err(_) => Err(NetworkError::Timeout),
        }
    }
}

fn is_retryable(error: &NetworkError) -> bool {
    match error {
        NetworkError::Transport(_) => true,
        // a timeout already consumed the whole engine budget
        NetworkError::Timeout => false,
        NetworkError::Status { status, .. } => *status == 429 || *status >= 500,
        NetworkError::Build(_) | NetworkError::Blocked | NetworkError::TooLarge => false,
    }
}

/// The default user agent: the honest one in honest mode, otherwise (the
/// default, also before any settings were read) upstream's `searxng_useragent`.
pub fn default_user_agent() -> String {
    if let Some(Some(agent)) = PROCESS_HONEST_AGENT.get() {
        return agent.clone();
    }
    build_user_agents()
        .into_iter()
        .next()
        .unwrap_or_else(|| "rustxng".to_string())
}

/// Build the user agent pool like upstream's `data/useragents.json`.
fn build_user_agents() -> Vec<String> {
    let mut agents = Vec::new();
    for os in USER_AGENT_OS {
        for version in USER_AGENT_VERSIONS {
            agents.push(
                USER_AGENT_TEMPLATE
                    .replace("{os}", os)
                    .replace("{version}", version),
            );
        }
    }
    agents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_body_limit_stops_an_oversized_response() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let chunk = vec![b'x'; 1024 * 1024];
            for _ in 0..9 {
                if socket.write_all(b"100000\r\n").is_err()
                    || socket.write_all(&chunk).is_err()
                    || socket.write_all(b"\r\n").is_err()
                {
                    return;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n");
        });
        let network = Network::from_settings(&Settings::defaults()).unwrap();
        // Private send is exercised only inside this test, never via a production bypass.
        let result = network
            .send(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                &EngineRequest::get(format!("http://{address}")),
                Duration::from_secs(8),
            )
            .await;
        assert!(matches!(result, Err(NetworkError::TooLarge)));
        server.join().unwrap();
    }

    #[test]
    fn user_agents_are_diverse_and_plausible() {
        let network = Network {
            public_client: crate::public_web::client().unwrap(),
            outgoing: crate::settings::Outgoing::default(),
            user_agents: build_user_agents(),
            suffix: String::new(),
            honest_agent: None,
            retries: 0,
            counter: AtomicUsize::new(0),
        };
        let first = network.user_agent();
        let second = network.user_agent();
        assert!(first.starts_with("Mozilla/5.0"));
        assert_ne!(first, second);
    }

    #[test]
    fn browser_identity_is_the_default() {
        let settings = Settings::defaults();
        assert_eq!(settings.outgoing.identity, "browser");
        assert_eq!(crate::settings::Outgoing::default().identity, "browser");
        assert_eq!(honest_agent_for(&settings.outgoing), None);
    }

    #[test]
    fn honest_identity_is_fixed() {
        let mut settings = Settings::defaults();
        settings.outgoing.identity = "honest".into();
        let network = Network::from_settings(&settings).unwrap();
        let first = network.user_agent();
        assert_eq!(first, network.user_agent());
        assert!(first.starts_with("TalmudResearch/"), "{first}");
        assert!(!first.contains("Mozilla"), "{first}");
        assert!(first.contains("+https://github.com/aoaibiz/fast-research"));
    }

    #[test]
    fn browser_identity_restores_the_rotation() {
        let browser = crate::settings::Outgoing {
            identity: "browser".into(),
            ..Default::default()
        };
        assert_eq!(honest_agent_for(&browser), None);
        let outgoing = crate::settings::Outgoing {
            identity: "honest".into(),
            useragent: "ExampleBot/1.0 (+https://example.org/bot)".into(),
            useragent_suffix: "lab".into(),
            ..Default::default()
        };
        assert_eq!(
            honest_agent_for(&outgoing).as_deref(),
            Some("ExampleBot/1.0 (+https://example.org/bot) lab")
        );
    }

    #[tokio::test]
    async fn honest_mode_overrides_an_engine_user_agent_on_the_wire() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let read = socket.read(&mut buffer).unwrap();
            let head = String::from_utf8_lossy(&buffer[..read]).to_lowercase();
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
            head
        });
        let mut settings = Settings::defaults();
        settings.outgoing.identity = "honest".into();
        let network = Network::from_settings(&settings).unwrap();
        let mut request = EngineRequest::get(format!("http://{address}"));
        request.set_header("User-Agent", "Mozilla/5.0");
        // Private send is exercised only inside this test, never via a production bypass.
        let result = network
            .send(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                &request,
                Duration::from_secs(8),
            )
            .await;
        assert!(result.is_ok());
        let head = server.join().unwrap();
        assert!(head.contains("user-agent: talmudresearch/"), "{head}");
        assert!(!head.contains("user-agent: mozilla"), "{head}");
    }

    #[test]
    fn captcha_pages_are_recognised() {
        assert_eq!(
            classify_status(403, "please solve the captcha"),
            FailureKind::Captcha
        );
        assert_eq!(classify_status(403, "denied"), FailureKind::AccessDenied);
        assert_eq!(classify_status(429, ""), FailureKind::TooManyRequests);
        assert_eq!(classify_status(500, ""), FailureKind::Http);
    }
}
