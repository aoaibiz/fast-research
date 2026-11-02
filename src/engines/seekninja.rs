//! Seek Ninja engine, ported from searx/engines/seekninja.py.
//!
//! The endpoint is guarded by a SHA-256 proof of work. Upstream fetches the
//! challenge in request(); here challenge_id, nonce, k and indifficulty come
//! from the configuration, because the request stays synchronous.
// engine-id: seekninja

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "sen",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 10.0,
};

const BASE_URL: &str = "https://seek.ninja";
/// Upstream loops until the challenge is solved; this bound keeps it bounded.
const MAX_ATTEMPTS: u64 = 2_000_000;

/// Seek Ninja, a general search behind a proof of work.
pub struct SeekNinja {
    meta: EngineMeta,
}

impl SeekNinja {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for SeekNinja {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let id = self.meta.opt_str("challenge_id").unwrap_or_default();
        let nonce = self.meta.opt_str("nonce").unwrap_or_default();
        if id.is_empty() || nonce.is_empty() {
            return Err(EngineError::Message(
                "seekninja needs the challenge_id and nonce options".into(),
            ));
        }
        let k = self.meta.opt_i64("k").unwrap_or(1).clamp(1, 64) as u32;
        let difficulty = self.meta.opt_f64("indifficulty").unwrap_or(6.0);
        let answers = solve(nonce, k, difficulty)?;
        let answers: Vec<String> = answers.iter().map(u64::to_string).collect();
        let adult = match query.safesearch {
            0 => "off",
            1 => "moderate",
            _ => "strict",
        };
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("panswers", answers.join(",")),
            ("pid", id.to_string()),
            ("adult", adult.to_string()),
        ];
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let pairs = query_string(&pairs);
        let url = format!("{}/search-sse?{pairs}", base.trim_end_matches('/'));
        Ok(EngineRequest::get(url).header("Accept", "text/event-stream"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let mut results = Vec::new();
        for event in body.split("\n\n") {
            let Some(json) = event_payload(event) else {
                continue;
            };
            let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
                continue;
            };
            for item in items {
                let url = json_str(item, "url");
                let title = json_str(item, "title");
                if url.is_empty() || title.is_empty() || !url.starts_with("http") {
                    continue;
                }
                let content = strip_tags(&json_str(item, "blurb"));
                results.push(ResultBuilder::new(url, title).content(content).build());
            }
        }
        Ok(results)
    }
}

/// The result payload of one server-sent event, if it carries results.
fn event_payload(event: &str) -> Option<serde_json::Value> {
    let mut lines = event.lines();
    let (Some(name), Some(data), None) = (lines.next(), lines.next(), lines.next()) else {
        return None;
    };
    if !name.ends_with("resultsUpdate") {
        return None;
    }
    serde_json::from_str(data.strip_prefix("data: ")?).ok()
}

fn hex_string(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Upstream tries answers until k of them hash to the requested zero prefix.
fn solve(nonce: &str, k: u32, difficulty: f64) -> Result<Vec<u64>, EngineError> {
    if !(0.0..=8.0).contains(&difficulty) {
        return Err(EngineError::Message("seekninja: invalid difficulty".into()));
    }
    let leading = difficulty as usize;
    let fraction = difficulty - leading as f64;
    let prefix = "0".repeat(leading);
    // Upstream: maxNib = 15 - int(frac * 16), and 15 when there is no fraction.
    let max_nibble = 15 - (fraction * 16.0) as u32;
    let wanted = k as usize;
    let mut solutions: Vec<u64> = Vec::new();
    let mut answer: u64 = 0;
    while solutions.len() < wanted && answer < MAX_ATTEMPTS {
        let digest = Sha256::digest(format!("{nonce}{answer}").as_bytes());
        let hash = hex_string(&digest);
        if hash.starts_with(&prefix) {
            let nibble = char::from(hash.as_bytes()[leading]).to_digit(16);
            if nibble.unwrap_or(16) <= max_nibble {
                solutions.push(answer);
            }
        }
        answer += 1;
    }
    if solutions.len() < wanted {
        return Err(EngineError::Message(
            "seekninja: the proof of work was not solved".into(),
        ));
    }
    Ok(solutions)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"event: resultsUpdate
data: {"results": [{"url": "https://example.test/a", "title": "First", "blurb": "<b>Hello</b> world"}]}

event: resultsUpdate
data: {"results": [{"url": "javascript:void(0)", "title": "Bad", "blurb": ""}, {"url": "https://example.test/b", "title": "Second", "blurb": "Plain"}]}"##;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("seekninja", "seekninja", "sen", &["general"]);
        let extra = &mut config.extra;
        extra.insert("challenge_id".into(), "cid".into());
        extra.insert("nonce".into(), "abc".into());
        extra.insert("k".into(), toml::Value::Integer(2));
        extra.insert("indifficulty".into(), toml::Value::Float(1.0));
        config
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn solves_the_proof_of_work() {
        assert_eq!(solve("abc", 2, 1.0).unwrap(), vec![26, 30]);
        assert_eq!(solve("abc", 3, 0.0).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn builds_the_event_stream_request() {
        let engine = SeekNinja::from_config(&config()).unwrap();
        let request = engine.build_request(&query(), 1).unwrap();
        assert!(request.url.starts_with("https://seek.ninja/search-sse?"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("panswers="));
        assert!(request.url.contains("pid=cid"));
        assert!(request.url.contains("adult=off"));
        let bare = EngineConfig::new("seekninja", "seekninja", "sen", &["general"]);
        let bare = SeekNinja::from_config(&bare).unwrap();
        assert!(bare.build_request(&query(), 1).is_err());
    }

    #[test]
    fn parses_the_result_events() {
        let engine = SeekNinja::from_config(&config()).unwrap();
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine.parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(first) => {
                assert_eq!(first.url, "https://example.test/a");
                assert_eq!(first.title, "First");
                assert_eq!(first.content, "Hello world");
            }
            _ => panic!("expected a result"),
        }
        let plain = EngineResponse::new(200, BASE_URL, b"event: ping\ndata: {}\n\n".to_vec());
        assert!(engine.parse(&plain, &query()).unwrap().is_empty());
    }
}
