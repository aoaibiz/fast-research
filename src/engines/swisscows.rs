//! Swisscows engine, ported from searx/engines/swisscows.py (web search).
// engine-id: swisscows

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::engine::util::*;
use crate::engine::{Engine, EngineDefaults, EngineError, EngineItem, EngineMeta};
use crate::engine::{EngineRequest, EngineResponse};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "sw",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.swisscows.com";
const NONCE_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
const REGIONS: &[&str] = &[
    "AR", "AU", "AT", "BE", "BR", "CA", "CL", "CN", "DK", "FI", "FR", "DE", "HK", "HU", "IN", "ID",
    "IT", "JP", "KR", "LV", "MY", "MX", "NL", "NZ", "NO", "PH", "PL", "PT", "RU", "SA", "ZA", "ES",
    "SE", "CH", "TW", "TR", "UA", "GB", "US",
];
const SUBTAGS: &[(&str, &str)] = &[("en", "en-US"), ("ja", "ja-JP"), ("zh", "zh-CN")];

/// Swisscows web search.
pub struct Swisscows {
    meta: EngineMeta,
}

impl Swisscows {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

/// The locale Swisscows understands for a SearXNG locale (upstream function).
fn appropriate_locale(locale: &str) -> String {
    let locale = locale.trim();
    if locale.is_empty() || locale == "all" {
        return "en-US".to_string();
    }
    let parts: Vec<&str> = locale.split('-').collect();
    let region = parts.get(1).copied().unwrap_or_default();
    // upstream returns a plain language tag unchanged
    if parts.len() == 1 || REGIONS.contains(&region) {
        return locale.to_string();
    }
    let upper = parts[0].to_uppercase();
    if REGIONS.contains(&upper.as_str()) {
        return format!("{}-{upper}", parts[0]);
    }
    SUBTAGS
        .iter()
        .find(|(code, _)| *code == parts[0])
        .map_or_else(|| "en-US".to_string(), |(_, tag)| (*tag).to_string())
}

/// A nonce for one request, derived from the clock and a counter.
fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now();
    let nanos = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(&nanos.to_be_bytes()[..]);
    hasher.update(&count.to_be_bytes()[..]);
    hasher
        .finalize()
        .iter()
        .map(|byte| NONCE_ALPHABET[usize::from(*byte) % NONCE_ALPHABET.len()] as char)
        .collect()
}

/// Caesar shift that inverts the case of every letter (upstream helper).
fn shift(text: &str, offset: usize) -> String {
    text.chars()
        .map(|character| {
            let upper = character.to_ascii_uppercase();
            if !upper.is_ascii_uppercase() {
                return character;
            }
            let index = usize::from(upper as u8 - b'A');
            let shifted = (b'A' + ((index + offset) % 26) as u8) as char;
            if character.is_ascii_uppercase() {
                shifted.to_ascii_lowercase()
            } else {
                shifted
            }
        })
        .collect()
}

fn base64_url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let combined = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for position in 0..4 {
            if (position == 2 && chunk.len() < 2) || (position == 3 && chunk.len() < 3) {
                continue;
            }
            output.push(TABLE[((combined >> (18 - 6 * position)) & 63) as usize] as char);
        }
    }
    output
}

fn base64_url_decode(encoded: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in encoded.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
        }
    }
    Some(output)
}

/// The X-Request-Signature of a request (upstream function).
fn signature(nonce: &str, args: &[(&str, String)]) -> String {
    let mut sorted = args.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let query = sorted
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/v5/web/search?{query}{}", shift(nonce, 13));
    base64_url(Sha256::digest(path.as_bytes()).as_slice())
}

impl Engine for Swisscows {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let count = self
            .meta
            .opt_i64("results_per_page")
            .unwrap_or(20)
            .clamp(1, 20);
        let freshness = match query.time_range {
            Some(TimeRange::Day) => "Day",
            Some(TimeRange::Week) => "Week",
            Some(TimeRange::Month) => "Month",
            Some(TimeRange::Year) => "Year",
            None => "All",
        };
        let args: Vec<(&str, String)> = vec![
            ("freshness", freshness.to_string()),
            ("itemsCount", count.to_string()),
            ("locale", appropriate_locale(&query.locale)),
            ("offset", (i64::from(pageno.max(1) - 1) * count).to_string()),
            ("query", query.query_text()),
            ("spellcheck", "True".to_string()),
        ];
        let nonce = nonce();
        let signature = signature(&nonce, &args);
        let mut request =
            EngineRequest::get(format!("{base}/v5/web/search?{}", query_string(&args)));
        request.set_header("X-Request-Nonce", &nonce);
        request.set_header("X-Request-Signature", &signature);
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let mut json = response.json()?;
        if let Some(payload) = json_path(&json, "payload").and_then(|value| value.as_str()) {
            let encoded = payload.split('.').nth(1).unwrap_or(payload);
            let bytes = base64_url_decode(encoded)
                .ok_or_else(|| EngineError::Parse("swisscows: invalid payload".into()))?;
            json = serde_json::from_slice(&bytes).map_err(|error| {
                EngineError::Parse(format!("swisscows: invalid payload: {error}"))
            })?;
        }
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            if json_str(item, "type") != "WebPage" {
                continue;
            }
            let (url, title) = (json_str(item, "url"), json_str(item, "name"));
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let mut builder =
                ResultBuilder::new(url, title).content(strip_tags(&json_str(item, "description")));
            let thumbnail = json_str(item, "thumbnail.url");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Swisscows> {
        Swisscows::from_config(&EngineConfig::new(
            "swisscows",
            "swisscows",
            "sw",
            &["general"],
        ))
        .expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn helpers_and_request_match_upstream() {
        assert_eq!(shift("Abc", 13), "nOP");
        assert_eq!(
            base64_url_decode(&base64_url(b"rust")),
            Some(b"rust".to_vec())
        );
        // upstream keeps a language-only tag as-is
        assert_eq!(appropriate_locale("de"), "de");
        let mut request_query = query("rust");
        request_query.locale = "de-DE".to_string();
        request_query.time_range = Some(TimeRange::Day);
        let request = engine().build_request(&request_query, 2).unwrap();
        assert!(request.url.contains("freshness=Day") && request.url.contains("itemsCount=20"));
        assert!(request.url.contains("locale=de-DE") && request.url.contains("offset=20"));
        assert!(request.url.contains("query=rust") && request.url.contains("spellcheck=True"));
        assert_eq!(
            request.headers["x-request-nonce"].to_str().unwrap().len(),
            32
        );
        assert_eq!(
            request.headers["x-request-signature"]
                .to_str()
                .unwrap()
                .len(),
            43
        );
    }

    #[test]
    fn parses_a_jwt_payload() {
        let inner = r##"{"items":[{"type":"WebPage","url":"https://www.rust-lang.org/","name":"Rust","description":"A <b>language</b>."}]}"##;
        let payload = format!("x.{}.y", base64_url(inner.as_bytes()));
        let body = serde_json::json!({"payload": payload}).to_string();
        let response = EngineResponse::new(200, BASE_URL, body.into_bytes());
        let items = engine().parse(&response, &query("rust")).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "A language.");
            }
            _ => panic!("expected a result"),
        }
        let broken = EngineResponse::new(200, BASE_URL, b"not json".to_vec());
        assert!(matches!(
            engine().parse(&broken, &query("rust")),
            Err(EngineError::Parse(_))
        ));
    }
}
