//! GMX engine, ported from searx/engines/gmx.py.
//!
//! Upstream first loads /web/result to scrape the per page hash that /desk
//! expects. build_request cannot issue that extra request, so the hash comes
//! from the page_hash setting, or from the request time when it is not set.

// engine-id: gmx

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "gmx",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://search.gmx.com";
const ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// The GMX web search.
pub struct Gmx {
    meta: EngineMeta,
}

impl Gmx {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Gmx {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        // The last digit is dropped, exactly like upstream's time based parameter.
        let now = chrono::Utc::now().timestamp() / 10;
        let page_hash = match self.meta.opt_str("page_hash").map(str::trim) {
            Some(value) if !value.is_empty() => value.to_string(),
            _ => now.to_string(),
        };
        let mut pairs = vec![
            ("lang", "en".to_string()),
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("h", page_hash),
            ("t", now.to_string()),
        ];
        if query.safesearch > 0 {
            pairs.push(("family", "True".to_string()));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "d",
                TimeRange::Week => "w",
                TimeRange::Month => "m",
                TimeRange::Year => "y",
            };
            pairs.push(("time", value.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{base}/desk?{}", query_string(&pairs)))
                .header("User-Agent", USER_AGENT)
                .header("Accept", ACCEPT)
                .header("Accept-Language", "en-US,en;q=0.9")
                .header("Connection", "keep-alive")
                .header("Referer", base),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("spam-messages") {
            return Err(EngineError::Captcha);
        }
        let json = response.json()?;
        let Some(results_node) = json_path(&json, "results") else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        let suggestions = json_path(results_node, "rs").and_then(|value| value.as_array());
        if let Some(suggestions) = suggestions {
            for suggestion in suggestions {
                let label = json_str(suggestion, "t");
                if !label.is_empty() {
                    items.push(EngineItem::Suggestion(label));
                }
            }
        }
        let hits = json_path(results_node, "hits").and_then(|value| value.as_array());
        let Some(hits) = hits else {
            return Ok(items);
        };
        for hit in hits {
            let url = json_str(hit, "u");
            let title = strip_tags(&json_str(hit, "t"));
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            items.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(hit, "s")))
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
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"{"results":{
      "rs":[{"t":"rust language"}],
      "hits":[
        {"u":"https://www.rust-lang.org/","t":"The <b>Rust</b> language","s":"Fast and <em>safe</em>."},
        {"u":"ftp://example.com/file","t":"bad url","s":""},
        {"u":"https://example.com/empty","t":"","s":""}
      ]}}"##;

    fn engine() -> Arc<Gmx> {
        Gmx::from_config(&EngineConfig::new("gmx", "gmx", "gmx", &["general"])).unwrap()
    }

    fn search() -> Query {
        Query::parse("rust", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_desk_request() {
        let mut query = search();
        query.safesearch = 1;
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with("https://search.gmx.com/desk?"));
        assert!(request.url.contains("lang=en"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("family=True"));
        assert!(request.url.contains("time=w"));
        assert!(request.url.contains("h="));
        assert!(request.url.contains("t="));
        let referer = request.headers.get("referer").unwrap().to_str().unwrap();
        assert_eq!(referer, "https://search.gmx.com");
    }

    #[test]
    fn page_hash_setting_is_used() {
        let mut config = EngineConfig::new("gmx", "gmx", "gmx", &["general"]);
        let page_hash = toml::Value::String("abc123".to_string());
        config.extra.insert("page_hash".to_string(), page_hash);
        let engine = Gmx::from_config(&config).unwrap();
        let request = engine.build_request(&search(), 1).unwrap();
        assert!(request.url.contains("h=abc123"));
    }

    #[test]
    fn parses_suggestions_and_hits() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Suggestion(suggestion) = &items[0] else {
            panic!("expected a suggestion")
        };
        assert_eq!(suggestion, "rust language");
        let EngineItem::Result(result) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://www.rust-lang.org/");
        assert_eq!(result.title, "The Rust language");
        assert_eq!(result.content, "Fast and safe.");
    }

    #[test]
    fn captcha_pages_are_reported() {
        let body = b"<div id=\"spam-messages\"></div>".to_vec();
        let response = EngineResponse::new(200, BASE_URL, body);
        let outcome = engine().parse(&response, &search());
        assert!(matches!(outcome, Err(EngineError::Captcha)));
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&empty, &search()).unwrap().is_empty());
    }
}
