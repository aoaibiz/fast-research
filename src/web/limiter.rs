//! Bot detection / IP rate limitation, ported from upstream's
//! `searx/limiter.py` and `searx/botdetection/*`.
//!
//! Upstream's limiter is a *before_request* filter.  This port keeps the same
//! observable behaviour for the parts that do not need a Valkey database:
//!
//! * real client IP resolution behind trusted proxies (`trusted_proxies.py`),
//! * the pass/block IP lists (`ip_lists.py`),
//! * the sliding window IP limit with burst, long and API windows
//!   (`ip_limit.py`),
//! * the link token challenge (`link_token.py`), implemented as a stateless
//!   HMAC cookie instead of a Valkey ping key,
//! * the public instance hardening of `limiter.initialize()`.
//!
//! The probe header methods (`http_accept*`, `http_user_agent`,
//! `http_sec_fetch`) and the Valkey backed token/ping storage are intentionally
//! not ported; see the module report in the repository for the exact list.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use sha2::{Digest, Sha256};

use crate::settings::{BotDetection, Settings};

/// Time (sec) before the sliding window for *burst* requests expires.
const BURST_WINDOW: Duration = Duration::from_secs(20);
/// Maximum requests from one network in `BURST_WINDOW`.
const BURST_MAX: u32 = 15;
/// Maximum suspicious requests from one network in `BURST_WINDOW`.
const BURST_MAX_SUSPICIOUS: u32 = 2;

/// Time (sec) before the longer sliding window expires.
const LONG_WINDOW: Duration = Duration::from_secs(600);
/// Maximum requests from one network in `LONG_WINDOW`.
const LONG_MAX: u32 = 150;
/// Maximum suspicious requests from one network in `LONG_WINDOW`.
const LONG_MAX_SUSPICIOUS: u32 = 10;

/// Time (sec) before the sliding window for API requests expires.
const API_WINDOW: Duration = Duration::from_secs(3600);
/// Maximum API requests from one network in `API_WINDOW`.
const API_MAX: u32 = 4;

/// Time (sec) before the sliding window for one suspicious network expires.
const SUSPICIOUS_IP_WINDOW: Duration = Duration::from_secs(3600 * 24 * 30);
/// Maximum requests from one suspicious network in `SUSPICIOUS_IP_WINDOW`.
const SUSPICIOUS_IP_MAX: u32 = 3;

/// Lifetime (sec) of the HMAC link token cookie.
const TOKEN_LIVE_TIME: Duration = Duration::from_secs(600);
/// Name of the HMAC link token cookie.
const TOKEN_COOKIE: &str = "limiter_token";

/// The upper bound of entries kept per sliding window.  Upstream relies on
/// Valkey expiry; in memory we additionally bound a single flood.
const MAX_COUNTER_ENTRIES: usize = 4096;
/// The number of (network, counter) pairs kept before old windows are pruned.
const MAX_COUNTERS: usize = 10_000;

/// `100::/128` from :rfc:`6666`, used when no client address can be determined.
const BLACK_HOLE: IpAddr = IpAddr::V6(Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0));

/// Addresses of check.searx.space, upstream's `SEARXNG_ORG` pass list.
const SEARXNG_ORG: [&str; 2] = ["167.235.158.251", "2a01:04f8:1c1c:8fc2::/64"];

/// Limits used by the IP limit method.  They mirror the module constants in
/// `botdetection/ip_limit.py`; the legacy flat settings map onto the burst and
/// long window.
#[derive(Debug, Clone, Copy)]
struct Rules {
    burst_window: Duration,
    burst_max: u32,
    burst_max_suspicious: u32,
    long_window: Duration,
    long_max: u32,
    long_max_suspicious: u32,
    api_window: Duration,
    api_max: u32,
    suspicious_window: Duration,
    suspicious_max: u32,
    token_ttl: Duration,
    link_token: bool,
    filter_link_local: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            burst_window: BURST_WINDOW,
            burst_max: BURST_MAX,
            burst_max_suspicious: BURST_MAX_SUSPICIOUS,
            long_window: LONG_WINDOW,
            long_max: LONG_MAX,
            long_max_suspicious: LONG_MAX_SUSPICIOUS,
            api_window: API_WINDOW,
            api_max: API_MAX,
            suspicious_window: SUSPICIOUS_IP_WINDOW,
            suspicious_max: SUSPICIOUS_IP_MAX,
            token_ttl: TOKEN_LIVE_TIME,
            link_token: false,
            filter_link_local: false,
        }
    }
}

impl Rules {
    /// The old flat `server.limiter = true` semantics: one window and limit.
    fn legacy(window: Duration, max_requests: u32) -> Self {
        let window = window.max(Duration::from_millis(1));
        let max_requests = max_requests.max(1);
        Self {
            burst_window: window,
            burst_max: max_requests,
            long_window: window,
            long_max: max_requests,
            api_max: max_requests,
            ..Self::default()
        }
    }
}

/// The kinds of sliding windows upstream keeps per client network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CounterKind {
    Api,
    Burst,
    Long,
    Suspicious,
    /// Burst window for clients without a valid link token.
    SuspiciousBurst,
    /// Long window for clients without a valid link token.
    SuspiciousLong,
}

impl CounterKind {
    fn window(self, rules: &Rules) -> Duration {
        match self {
            Self::Api => rules.api_window,
            Self::Burst | Self::SuspiciousBurst => rules.burst_window,
            Self::Long | Self::SuspiciousLong => rules.long_window,
            Self::Suspicious => rules.suspicious_window,
        }
    }
}

/// Key of a sliding window: (client network, limiter method).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CounterKey {
    network: String,
    kind: CounterKind,
}

impl CounterKey {
    fn new(network: &str, kind: CounterKind) -> Self {
        Self {
            network: network.to_string(),
            kind,
        }
    }
}

/// Counters reported to `/stats` (and the limiter debug log).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimiterStats {
    pub enabled: bool,
    /// Requests that passed the limiter.
    pub allowed: u64,
    /// Requests that were answered with a link token challenge.
    pub challenged: u64,
    /// Requests that were refused with 429 (or redirected by the suspicious window).
    pub blocked: u64,
    /// Number of (network, method) sliding windows currently tracked.
    pub tracked: usize,
}

/// The result of `Limiter::check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimiterVerdict {
    /// The request may continue.
    Allow,
    /// Refuse the request with 429 Too Many Requests.
    Blocked { retry_after: u64, message: String },
    /// Soft limit hit: answer 429, set the HMAC link token cookie and let the
    /// client retry with it.
    Challenge {
        retry_after: u64,
        message: String,
        set_cookie: String,
    },
    /// Too many requests from one suspicious network: upstream redirects to `/`.
    Redirect,
}

impl LimiterVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[derive(Debug, Default)]
struct LimiterState {
    counters: HashMap<CounterKey, VecDeque<Instant>>,
    stats: LimiterStats,
}

/// Port of upstream's bot detection limiter.
#[derive(Debug)]
pub struct Limiter {
    enabled: bool,
    rules: Rules,
    ipv4_prefix: u8,
    ipv6_prefix: u8,
    trusted_proxies: Vec<IpNet>,
    pass_ip: Vec<IpNet>,
    block_ip: Vec<IpNet>,
    searxng_org: Vec<IpNet>,
    pass_searxng_org: bool,
    secret_key: Vec<u8>,
    state: Mutex<LimiterState>,
}

impl Limiter {
    /// A limiter for `max_requests` requests per `window`, the old flat API.
    ///
    /// This keeps the legacy behaviour for callers and tests that do not build
    /// a full `[server.limiter]` configuration.
    pub fn new(enabled: bool, window: Duration, max_requests: u32) -> Self {
        Self::assemble(
            enabled,
            Rules::legacy(window, max_requests),
            &BotDetection::default(),
            &[],
            "",
        )
    }

    /// A disabled limiter (used when `server.limiter` is `false`).
    pub fn disabled() -> Self {
        Self::new(false, Duration::from_secs(60), 30)
    }

    /// Build the limiter from settings, honouring both the flat and the
    /// `[server.limiter]` table form.
    pub fn from_settings(settings: &Settings) -> Self {
        let public_instance = settings.is_public_instance();
        let (mut enabled, mut rules, botdetection, extra_trusted) =
            match settings.server.limiter_config() {
                Some(config) => {
                    let rules = Rules {
                        link_token: config.botdetection.ip_limit.link_token,
                        filter_link_local: config.botdetection.ip_limit.filter_link_local,
                        ..Rules::default()
                    };
                    (
                        config.enabled || public_instance,
                        rules,
                        config.botdetection.clone(),
                        config.trusted_proxies.clone(),
                    )
                }
                None => {
                    let rules = if public_instance {
                        // Stricter defaults on a public instance.
                        Rules::default()
                    } else {
                        Rules::legacy(
                            Duration::from_secs(settings.server.limiter_window),
                            settings.server.limiter_max,
                        )
                    };
                    (
                        settings.server.limiter_enabled() || public_instance,
                        rules,
                        BotDetection::default(),
                        Vec::new(),
                    )
                }
            };

        if public_instance {
            // Upstream: `cfg.set('botdetection.ip_limit.link_token', True)`.
            rules.link_token = true;
            enabled = true;
        }

        Self::assemble(
            enabled,
            rules,
            &botdetection,
            &extra_trusted,
            &settings.server.secret_key,
        )
    }

    fn assemble(
        enabled: bool,
        rules: Rules,
        botdetection: &BotDetection,
        extra_trusted: &[String],
        secret_key: &str,
    ) -> Self {
        let trusted_proxies = botdetection
            .trusted_proxies
            .iter()
            .chain(extra_trusted.iter())
            .filter_map(|value| parse_network(value, "trusted_proxies"))
            .collect();
        let pass_ip = botdetection
            .ip_lists
            .pass_ip
            .iter()
            .filter_map(|value| parse_network(value, "pass_ip"))
            .collect();
        let block_ip = botdetection
            .ip_lists
            .block_ip
            .iter()
            .filter_map(|value| parse_network(value, "block_ip"))
            .collect();
        let searxng_org = SEARXNG_ORG
            .iter()
            .filter_map(|value| IpNet::parse(value))
            .collect();
        Self {
            enabled,
            rules,
            ipv4_prefix: botdetection.ipv4_prefix.min(32),
            ipv6_prefix: botdetection.ipv6_prefix.min(128),
            trusted_proxies,
            pass_ip,
            block_ip,
            searxng_org,
            pass_searxng_org: botdetection.ip_lists.pass_searxng_org,
            secret_key: secret_key.as_bytes().to_vec(),
            state: Mutex::new(LimiterState::default()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Upstream's `limiter.is_installed()`: the limiter is active.
    pub fn is_installed(&self) -> bool {
        self.enabled
    }

    /// Resolve the real client IP from the direct peer and the forwarding
    /// headers, like upstream's `ProxyFix`.
    ///
    /// `X-Forwarded-For` and `X-Real-IP` are only honoured when the direct
    /// peer is listed in `botdetection.trusted_proxies`.  Without headers the
    /// peer address is used; when even that is unknown, `100::` is returned.
    pub fn real_ip(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> IpAddr {
        let peer = peer.map(normalize_ip);
        let trusted = peer.is_some_and(|ip| self.is_trusted_proxy(ip));

        let mut forwarded = Vec::new();
        let mut forwarded_valid = true;
        if let Some(value) = headers.get("x-forwarded-for") {
            match value.to_str() {
                Ok(value) => {
                    for part in value.split(',') {
                        match part.trim().parse::<IpAddr>() {
                            Ok(ip) => forwarded.push(normalize_ip(ip)),
                            Err(_) => {
                                forwarded_valid = false;
                                break;
                            }
                        }
                    }
                }
                Err(_) => forwarded_valid = false,
            }
        }
        if !forwarded_valid {
            // Upstream discards the whole header when one entry is invalid.
            forwarded.clear();
        }

        let x_real_ip = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
            .map(normalize_ip);

        if trusted {
            if !forwarded.is_empty() {
                // Walk from the direct peer towards the client: the first
                // address that is not a trusted proxy is the client.
                for address in forwarded.iter().rev() {
                    if !self.is_trusted_proxy(*address) {
                        return *address;
                    }
                }
                // Every hop is a trusted proxy; upstream falls back to the first.
                return forwarded[0];
            }
            if let Some(address) = x_real_ip {
                return address;
            }
        }

        peer.unwrap_or(BLACK_HOLE)
    }

    /// Check a request against the limiter methods.
    ///
    /// `format` is the requested search format; every format except `html` is
    /// counted in the API window, like upstream's `request.args['format']`.
    pub fn check(&self, peer: Option<IpAddr>, headers: &HeaderMap, format: &str) -> LimiterVerdict {
        if !self.enabled {
            return LimiterVerdict::Allow;
        }

        let real_ip = self.real_ip(peer, headers);

        // ip_limit / filter_request: link-local networks are not monitored.
        if !self.rules.filter_link_local && is_link_local(real_ip) {
            tracing::debug!(%real_ip, "network is link-local, not monitored by the limiter");
            return self.allow();
        }

        // ip_lists: the pass list has priority over the block list.
        if self.pass_searxng_org
            && self
                .searxng_org
                .iter()
                .any(|network| network.contains(real_ip))
        {
            return self.allow();
        }
        if self.pass_ip.iter().any(|network| network.contains(real_ip)) {
            tracing::debug!(%real_ip, "client matches botdetection.ip_lists.pass_ip");
            return self.allow();
        }
        if let Some(network) = self
            .block_ip
            .iter()
            .find(|network| network.contains(real_ip))
        {
            let message = format!(
                "IP is on BLOCKLIST - IP matches {} in botdetection.ip_lists.block_ip",
                network
            );
            tracing::debug!(%real_ip, %message, "limiter blocked request");
            return self.deny(0, message);
        }

        let network = self.network(real_ip);
        let now = Instant::now();
        let mut state = self.state();
        state.cleanup(now, &self.rules);

        // ip_limit.API_WINDOW (only non-html formats).
        if format != "html" {
            let (count, retry_after) = bump(
                &mut state,
                &network,
                CounterKind::Api,
                self.rules.api_window,
                now,
            );
            if count > self.rules.api_max {
                state.stats.blocked += 1;
                tracing::debug!(%network, count, "too many requests in API_WINDOW");
                return LimiterVerdict::Blocked {
                    retry_after,
                    message: format!("too many requests in API_WINDOW ({})", self.rules.api_max),
                };
            }
        }

        if !self.rules.link_token {
            return self.limited(&mut state, &network, now);
        }

        // link_token: a valid token makes the request non-suspicious.
        if let Some(token) = cookie_token(headers) {
            if self.token_is_valid(&network, &token) {
                state
                    .counters
                    .remove(&CounterKey::new(&network, CounterKind::Suspicious));
                return self.limited(&mut state, &network, now);
            }
        }

        // Suspicious request: count it against the suspicious windows.
        let (suspicious, _) = bump(
            &mut state,
            &network,
            CounterKind::Suspicious,
            self.rules.suspicious_window,
            now,
        );
        if suspicious > self.rules.suspicious_max {
            state.stats.blocked += 1;
            tracing::debug!(
                %network,
                count = suspicious,
                "too many requests in SUSPICIOUS_IP_WINDOW, redirecting to /"
            );
            return LimiterVerdict::Redirect;
        }

        let (burst, retry_after) = bump(
            &mut state,
            &network,
            CounterKind::SuspiciousBurst,
            self.rules.burst_window,
            now,
        );
        if burst > self.rules.burst_max_suspicious {
            state.stats.challenged += 1;
            tracing::debug!(%network, count = burst, "challenging suspicious request");
            return self.challenge(&network, retry_after);
        }

        let (long, retry_after) = bump(
            &mut state,
            &network,
            CounterKind::SuspiciousLong,
            self.rules.long_window,
            now,
        );
        if long > self.rules.long_max_suspicious {
            state.stats.challenged += 1;
            tracing::debug!(%network, count = long, "challenging suspicious request");
            return self.challenge(&network, retry_after);
        }

        state.stats.allowed += 1;
        LimiterVerdict::Allow
    }

    /// The vanilla ip_limit path (no link token).
    fn limited(&self, state: &mut LimiterState, network: &str, now: Instant) -> LimiterVerdict {
        let (burst, retry_after) = bump(
            state,
            network,
            CounterKind::Burst,
            self.rules.burst_window,
            now,
        );
        if burst > self.rules.burst_max {
            state.stats.blocked += 1;
            tracing::debug!(%network, count = burst, "too many requests in BURST_WINDOW");
            return LimiterVerdict::Blocked {
                retry_after,
                message: format!(
                    "too many requests in BURST_WINDOW (BURST_MAX = {})",
                    self.rules.burst_max
                ),
            };
        }

        let (long, retry_after) = bump(
            state,
            network,
            CounterKind::Long,
            self.rules.long_window,
            now,
        );
        if long > self.rules.long_max {
            state.stats.blocked += 1;
            tracing::debug!(%network, count = long, "too many requests in LONG_WINDOW");
            return LimiterVerdict::Blocked {
                retry_after,
                message: format!(
                    "too many requests in LONG_WINDOW (LONG_MAX = {})",
                    self.rules.long_max
                ),
            };
        }

        state.stats.allowed += 1;
        LimiterVerdict::Allow
    }

    fn allow(&self) -> LimiterVerdict {
        self.state().stats.allowed += 1;
        LimiterVerdict::Allow
    }

    fn deny(&self, retry_after: u64, message: String) -> LimiterVerdict {
        self.state().stats.blocked += 1;
        LimiterVerdict::Blocked {
            retry_after,
            message,
        }
    }

    fn challenge(&self, network: &str, retry_after: u64) -> LimiterVerdict {
        LimiterVerdict::Challenge {
            retry_after: retry_after.max(1),
            message: "too many requests - reload the page to pass the limiter check".into(),
            set_cookie: self.token_cookie(network),
        }
    }

    fn state(&self) -> MutexGuard<'_, LimiterState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of tracked (network, method) windows (used by `/stats`).
    pub fn tracked(&self) -> usize {
        self.state().counters.len()
    }

    /// Counters for `/stats` and the limiter debug log.
    pub fn stats(&self) -> LimiterStats {
        let state = self.state();
        LimiterStats {
            enabled: self.enabled,
            tracked: state.counters.len(),
            ..state.stats
        }
    }

    fn is_trusted_proxy(&self, ip: IpAddr) -> bool {
        self.trusted_proxies
            .iter()
            .any(|network| network.contains(ip))
    }

    /// The (client) network of `ip` as configured by `ipv4_prefix` /
    /// `ipv6_prefix`, rendered like Python's `ip_network(...).compressed`.
    fn network(&self, ip: IpAddr) -> String {
        let prefix = if ip.is_ipv4() {
            self.ipv4_prefix
        } else {
            self.ipv6_prefix
        };
        format!("{}/{}", mask_ip(ip, prefix), prefix)
    }

    fn token_cookie(&self, network: &str) -> String {
        let expires = now_unix().saturating_add(self.rules.token_ttl.as_secs());
        let signature = self.sign(network, expires);
        format!(
            "{TOKEN_COOKIE}={expires}.{signature}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax",
            self.rules.token_ttl.as_secs()
        )
    }

    fn sign(&self, network: &str, expires: u64) -> String {
        let mut mac = HmacSha256::new(&self.secret_key);
        mac.update(network.as_bytes());
        mac.update(b"|");
        mac.update(expires.to_string().as_bytes());
        hex(&mac.finalize())
    }

    fn token_is_valid(&self, network: &str, token: &str) -> bool {
        let Some((expires, signature)) = token.split_once('.') else {
            return false;
        };
        let Ok(expires) = expires.parse::<u64>() else {
            return false;
        };
        let now = now_unix();
        // Expired tokens and tokens that claim a longer life than we issue are
        // both invalid.
        if expires <= now || expires > now.saturating_add(self.rules.token_ttl.as_secs() + 60) {
            return false;
        }
        let expected = self.sign(network, expires);
        constant_time_eq(expected.as_bytes(), signature.as_bytes())
    }
}

/// An extractor for the direct peer address.
///
/// `Some` when the server was started with
/// `into_make_service_with_connect_info`; `None` in unit tests that call the
/// router directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientAddr(pub Option<IpAddr>);

impl<S> FromRequestParts<S> for ClientAddr
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip()),
        ))
    }
}

fn bump(
    state: &mut LimiterState,
    network: &str,
    kind: CounterKind,
    window: Duration,
    now: Instant,
) -> (u32, u64) {
    let key = CounterKey::new(network, kind);
    let queue = state.counters.entry(key).or_default();
    while let Some(front) = queue.front() {
        if now.duration_since(*front) > window {
            queue.pop_front();
        } else {
            break;
        }
    }
    queue.push_back(now);
    if queue.len() > MAX_COUNTER_ENTRIES {
        queue.pop_front();
    }
    let count = queue.len() as u32;
    let retry_after = queue
        .front()
        .map(|oldest| {
            let remaining = window.saturating_sub(now.duration_since(*oldest));
            // Round up: the client has to wait at least one second.
            (remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)).max(1)
        })
        .unwrap_or(1);
    (count, retry_after)
}

impl LimiterState {
    fn cleanup(&mut self, now: Instant, rules: &Rules) {
        if self.counters.len() <= MAX_COUNTERS {
            return;
        }
        self.counters.retain(|key, queue| {
            let window = key.kind.window(rules);
            while let Some(front) = queue.front() {
                if now.duration_since(*front) > window {
                    queue.pop_front();
                } else {
                    break;
                }
            }
            !queue.is_empty()
        });
    }
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    for value in headers.get_all(header::COOKIE).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for part in value.split(';') {
            if let Some((name, value)) = part.trim().split_once('=')
                && name.trim() == TOKEN_COOKIE
            {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

/// Minimal HMAC-SHA256, so the token does not need another dependency.
struct HmacSha256 {
    inner: Sha256,
    outer_key: [u8; 64],
}

impl HmacSha256 {
    fn new(key: &[u8]) -> Self {
        let mut block = [0u8; 64];
        if key.len() > block.len() {
            let digest = Sha256::digest(key);
            block[..digest.len()].copy_from_slice(&digest);
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        let mut outer_key = [0u8; 64];
        for (index, byte) in block.iter().enumerate() {
            inner.update([*byte ^ 0x36]);
            outer_key[index] = *byte ^ 0x5c;
        }
        Self { inner, outer_key }
    }

    fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    fn finalize(self) -> [u8; 32] {
        let inner = self.inner.finalize();
        let mut outer = Sha256::new();
        outer.update(self.outer_key);
        outer.update(inner);
        let digest = outer.finalize();
        let mut result = [0u8; 32];
        result.copy_from_slice(&digest);
        result
    }
}

/// A parsed IPv4 or IPv6 network (CIDR matching for trusted proxies/lists).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix.parse::<u8>().ok()?)),
            None => (value, None),
        };
        let addr = normalize_ip(address.parse::<IpAddr>().ok()?);
        let maximum = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(maximum);
        if prefix > maximum {
            return None;
        }
        Some(Self {
            addr: mask_ip(addr, prefix),
            prefix,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        let ip = normalize_ip(ip);
        self.addr.is_ipv4() == ip.is_ipv4() && mask_ip(ip, self.prefix) == self.addr
    }
}

impl std::fmt::Display for IpNet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.addr, self.prefix)
    }
}

fn parse_network(value: &str, kind: &str) -> Option<IpNet> {
    match IpNet::parse(value) {
        Some(network) => Some(network),
        None => {
            tracing::warn!(%value, %kind, "ignoring invalid IP network in limiter configuration");
            None
        }
    }
}

/// Treat IPv4 mapped IPv6 addresses (`::ffff:192.0.2.1`) as IPv4.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        IpAddr::V4(_) => ip,
    }
}

fn mask_ip(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(address) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix.min(32))
            };
            IpAddr::V4(Ipv4Addr::from(u32::from(address) & mask))
        }
        IpAddr::V6(address) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix.min(128))
            };
            IpAddr::V6(Ipv6Addr::from(u128::from(address) & mask))
        }
    }
}

fn is_link_local(ip: IpAddr) -> bool {
    match normalize_ip(ip) {
        IpAddr::V4(address) => {
            let octets = address.octets();
            octets[0] == 169 && octets[1] == 254
        }
        IpAddr::V6(address) => address.segments()[0] & 0xffc0 == 0xfe80,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("valid test address")
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(*name, HeaderValue::from_str(value).expect("valid header"));
        }
        headers
    }

    fn rules() -> Rules {
        Rules {
            burst_window: Duration::from_secs(60),
            burst_max: 2,
            burst_max_suspicious: 1,
            long_window: Duration::from_secs(600),
            long_max: 100,
            long_max_suspicious: 100,
            api_window: Duration::from_secs(3600),
            api_max: 100,
            suspicious_window: Duration::from_secs(60),
            suspicious_max: 100,
            token_ttl: Duration::from_secs(600),
            link_token: false,
            filter_link_local: true,
        }
    }

    fn test_limiter(rules: Rules, trusted_proxies: &[&str]) -> Limiter {
        Limiter {
            enabled: true,
            rules,
            ipv4_prefix: 32,
            ipv6_prefix: 48,
            trusted_proxies: trusted_proxies
                .iter()
                .filter_map(|value| IpNet::parse(value))
                .collect(),
            pass_ip: Vec::new(),
            block_ip: Vec::new(),
            searxng_org: Vec::new(),
            pass_searxng_org: false,
            secret_key: b"test-secret".to_vec(),
            state: Mutex::new(LimiterState::default()),
        }
    }

    #[test]
    fn disabled_limiter_allows_everything() {
        let limiter = Limiter::disabled();
        for _ in 0..100 {
            assert_eq!(
                limiter.check(Some(ip("203.0.113.7")), &HeaderMap::new(), "html"),
                LimiterVerdict::Allow
            );
        }
        assert!(!limiter.is_enabled());
        assert_eq!(limiter.stats().allowed, 0);
    }

    #[test]
    fn x_forwarded_for_is_used_only_from_a_trusted_proxy() {
        let limiter = test_limiter(rules(), &["10.0.0.0/8"]);
        let forwarded = headers(&[("x-forwarded-for", "203.0.113.9, 10.1.2.3")]);

        // The direct peer is a trusted proxy: use the first untrusted hop.
        assert_eq!(
            limiter.real_ip(Some(ip("10.1.2.3")), &forwarded),
            ip("203.0.113.9")
        );

        // An untrusted peer cannot spoof its address with the header.
        assert_eq!(
            limiter.real_ip(Some(ip("198.51.100.7")), &forwarded),
            ip("198.51.100.7")
        );

        // Trusted peer without X-Forwarded-For falls back to X-Real-IP.
        let real_ip = headers(&[("x-real-ip", "203.0.113.44")]);
        assert_eq!(
            limiter.real_ip(Some(ip("10.1.2.3")), &real_ip),
            ip("203.0.113.44")
        );
        assert_eq!(
            limiter.real_ip(Some(ip("198.51.100.7")), &real_ip),
            ip("198.51.100.7")
        );
    }

    #[test]
    fn ipv4_cidr_matching_is_inclusive() {
        let network = IpNet::parse("192.0.2.0/24").expect("valid network");
        assert!(network.contains(ip("192.0.2.0")));
        assert!(network.contains(ip("192.0.2.255")));
        assert!(!network.contains(ip("192.0.3.0")));
        // IPv4 mapped IPv6 addresses are treated as IPv4.
        assert!(network.contains(ip("::ffff:192.0.2.10")));

        let host = IpNet::parse("192.0.2.5/32").expect("valid host");
        assert!(host.contains(ip("192.0.2.5")));
        assert!(!host.contains(ip("192.0.2.6")));
        assert!(IpNet::parse("192.0.2.0/33").is_none());
        assert!(IpNet::parse("not-an-ip").is_none());
    }

    #[test]
    fn sliding_window_is_exhausted() {
        let limiter = test_limiter(rules(), &[]);
        let peer = Some(ip("203.0.113.7"));
        assert!(limiter.check(peer, &HeaderMap::new(), "html").is_allowed());
        assert!(limiter.check(peer, &HeaderMap::new(), "html").is_allowed());

        match limiter.check(peer, &HeaderMap::new(), "html") {
            LimiterVerdict::Blocked { retry_after, .. } => {
                assert!(retry_after >= 1, "a retry delay is reported");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }

        // Another client network is unaffected.
        assert!(
            limiter
                .check(Some(ip("198.51.100.1")), &HeaderMap::new(), "html")
                .is_allowed()
        );
        assert_eq!(limiter.stats().blocked, 1);
    }

    #[test]
    fn link_token_is_issued_and_verified() {
        let mut rules = rules();
        rules.link_token = true;
        let limiter = test_limiter(rules, &[]);
        let peer = Some(ip("203.0.113.7"));

        // First suspicious request is still below BURST_MAX_SUSPICIOUS.
        assert!(limiter.check(peer, &HeaderMap::new(), "html").is_allowed());

        let challenge = limiter.check(peer, &HeaderMap::new(), "html");
        let set_cookie = match challenge {
            LimiterVerdict::Challenge {
                set_cookie,
                retry_after,
                ..
            } => {
                assert!(retry_after >= 1);
                set_cookie
            }
            other => panic!("expected Challenge, got {other:?}"),
        };
        let token = set_cookie
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, value)| value.to_string())
            .expect("token cookie");
        let network = limiter.network(ip("203.0.113.7"));
        assert!(limiter.token_is_valid(&network, &token));
        assert!(!limiter.token_is_valid("198.51.100.0/32", &token));
        assert!(!limiter.token_is_valid(&network, "123.deadbeef"));

        // Presenting the token lets the request through the soft limit.
        let cookie = headers(&[("cookie", &format!("{TOKEN_COOKIE}={token}"))]);
        assert!(limiter.check(peer, &cookie, "html").is_allowed());
        assert_eq!(limiter.stats().challenged, 1);
    }

    #[test]
    fn public_instance_enables_link_tokens() {
        let mut settings = Settings::defaults();
        settings.server.public_instance = true;
        let limiter = Limiter::from_settings(&settings);
        assert!(limiter.is_enabled());
        assert!(limiter.rules.link_token);
        assert_eq!(limiter.rules.burst_max, BURST_MAX);
    }

    #[test]
    fn flat_and_table_configuration_are_accepted() {
        let flat: Settings = toml::from_str(
            r#"
            [server]
            limiter = true
            limiter_window = 30
            limiter_max = 3
            "#,
        )
        .expect("flat configuration");
        assert!(flat.server.limiter_enabled());
        let limiter = Limiter::from_settings(&flat);
        assert_eq!(limiter.rules.burst_max, 3);
        assert_eq!(limiter.rules.burst_window, Duration::from_secs(30));

        let table: Settings = toml::from_str(
            r#"
            [server.limiter]
            enabled = true
            trusted_proxies = ["10.0.0.0/8"]

            [server.limiter.botdetection]
            ipv4_prefix = 24

            [server.limiter.botdetection.ip_limit]
            link_token = true
            "#,
        )
        .expect("table configuration");
        assert!(table.server.limiter_enabled());
        let limiter = Limiter::from_settings(&table);
        assert!(limiter.is_enabled());
        assert!(limiter.rules.link_token);
        assert_eq!(limiter.ipv4_prefix, 24);
        assert!(limiter.is_trusted_proxy(ip("10.9.9.9")));
        assert!(!limiter.is_trusted_proxy(ip("203.0.113.9")));
    }
}
