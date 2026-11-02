//! DuckDuckGo Web engine, ported from `searx/engines/duckduckgo_web.py`.
//!
//! The general DuckDuckGo web search. Upstream first scrapes the first-page
//! link (and a challenge-cookie) from duckduckgo.com and caches it; the frozen
//! `build_request` interface cannot do that round trip, so the API link is read
//! from the `vqd` engine option and `o=json` is requested directly.
// engine-id: duckduckgo_web

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "ddgb",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://duckduckgo.com";
const LINKS_URL: &str = "https://links.duckduckgo.com/d.js";

/// DuckDuckGo general web search.
pub struct DuckDuckGoWeb {
    meta: EngineMeta,
}

impl DuckDuckGoWeb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for DuckDuckGoWeb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        if text.chars().count() >= 500 {
            return Err(EngineError::Message(
                "duckduckgo does not accept queries with 500 or more characters".into(),
            ));
        }
        // The `n` parameter advances the page on the API (upstream caches the
        // URLs of subsequent pages; here we approximate with `s`).
        let mut pairs = vec![
            ("o", "json".to_string()),
            ("q", text),
            ("t", "D".to_string()),
            ("l", "us-en".to_string()),
        ];
        if pageno > 1 {
            pairs.push(("s", ((pageno - 1) * 10).to_string()));
        } else {
            pairs.push(("s", "0".to_string()));
        }
        pairs.push(("a", "h_".to_string()));
        if let Some(vqd) = self.meta.opt_str("vqd") {
            if !vqd.is_empty() {
                pairs.push(("vqd", vqd.to_string()));
            }
        }
        let mut request = EngineRequest::get(format!("{LINKS_URL}?{}", query_string(&pairs)))
            .header("Accept", "*/*")
            .header("Sec-Fetch-Dest", "script")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Site", "same-site")
            .header("Referer", &format!("{BASE_URL}/"));
        request.set_header("User-Agent", "Mozilla/5.0");
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // DuckDuckGo sometimes answers with a challenge script instead of JSON.
        if body.contains("let jsa =") {
            return Err(EngineError::Captcha);
        }
        let json = response.json()?;
        let Some(results) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for result in results {
            if json_path(result, "u").is_none() {
                continue;
            }
            let url = json_str(result, "u");
            if url.is_empty() || !is_http_url(&url) {
                continue;
            }
            let title = strip_tags(&json_str(result, "t"));
            if title.is_empty() {
                continue;
            }
            items.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(result, "a")))
                    .build(),
            );
        }
        Ok(items)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_results() {
        let fixture = r#"{"results": [
            {"u": "https://www.rust-lang.org/", "t": "Rust <b>lang</b>", "a": "A language.<br/>"},
            {"u": "javascript:void(0)", "t": "Bad", "a": "x"},
            {"t": "No url", "a": "x"}
        ]}"#;
        let response = EngineResponse::new(200, LINKS_URL, fixture.as_bytes().to_vec());
        let engine = DuckDuckGoWeb::from_config(&EngineConfig::new(
            "duckduckgo_web",
            "duckduckgo_web",
            "ddgb",
            &["general"],
        ))
        .unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust lang");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn challenge_is_detected() {
        let response = EngineResponse::new(200, LINKS_URL, b"let jsa = 123;".to_vec());
        let engine = DuckDuckGoWeb::from_config(&EngineConfig::new(
            "duckduckgo_web",
            "duckduckgo_web",
            "ddgb",
            &["general"],
        ))
        .unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine.parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn build_request_targets_the_json_api() {
        let engine = DuckDuckGoWeb::from_config(&EngineConfig::new(
            "duckduckgo_web",
            "duckduckgo_web",
            "ddgb",
            &["general"],
        ))
        .unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://links.duckduckgo.com/d.js?")
        );
        assert!(request.url.contains("o=json"));
    }
}
