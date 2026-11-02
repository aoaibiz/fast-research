//! ChatNoir engine, ported from searx/engines/chatnoir.py.
//!
//! Upstream fetches a CSRF token and an API key from the front page when no
//! api_key is configured. The framework builds a single request per search, so
//! this port needs the api_key setting for the API to accept the request.

// engine-id: chatnoir

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "cha",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.chatnoir.eu";
const SEARCH_INDEX: &str = "cw22";
const PAGE_SIZE: i64 = 10;

/// The ChatNoir web corpus search.
pub struct Chatnoir {
    meta: EngineMeta,
}

impl Chatnoir {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Chatnoir {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let Some(key) = self
            .meta
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(EngineError::Message(
                "chatnoir requires an api_key: upstream scrapes one with two extra requests"
                    .to_string(),
            ));
        };
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let index = self
            .meta
            .opt_str("search_index")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(SEARCH_INDEX);
        let size = self
            .meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(PAGE_SIZE);
        let from = (pageno.max(1) - 1) as i64 * size;
        let body = serde_json::json!({
            "query": query.query_text(),
            "index": [index],
            "from": from,
            "size": size,
            "_extended_meta": true,
        });
        Ok(EngineRequest::post(format!("{base}/api/v1/_search"))
            .header("Authorization", &format!("Bearer {key}"))
            .header("Accept", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "results").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "target_uri");
            let title = strip_tags(&json_str(item, "title"));
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(item, "snippet")))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineRegistry, Method};

    const FIXTURE: &str = r##"{"results":[
      {"target_uri":"https://example.com/page","title":"The <b>Example</b> Page",
       "snippet":"A snippet with <em>markup</em>."},
      {"target_uri":"https://example.com/empty","title":"","snippet":"no title"},
      {"target_uri":"javascript:void(0)","title":"bad url","snippet":""}
    ]}"##;

    fn engine() -> Arc<Chatnoir> {
        let mut config = EngineConfig::new("chatnoir", "chatnoir", "cha", &["general"]);
        config.api_key = Some("secret".to_string());
        Chatnoir::from_config(&config).unwrap()
    }

    fn search() -> Query {
        Query::parse("common crawl", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_json_request() {
        let request = engine().build_request(&search(), 2).unwrap();
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.url, "https://www.chatnoir.eu/api/v1/_search");
        let body = request.json.as_ref().unwrap();
        assert_eq!(json_str(body, "query"), "common crawl");
        assert_eq!(json_str(body, "index.0"), "cw22");
        assert_eq!(json_str(body, "from"), "10");
        assert_eq!(json_str(body, "size"), "10");
        assert_eq!(json_str(body, "_extended_meta"), "true");
        let authorization = request.headers.get("authorization").unwrap();
        assert_eq!(authorization.to_str().unwrap(), "Bearer secret");
    }

    #[test]
    fn without_a_key_the_request_is_refused() {
        let config = EngineConfig::new("chatnoir", "chatnoir", "cha", &["general"]);
        let engine = Chatnoir::from_config(&config).unwrap();
        assert!(engine.build_request(&search(), 1).is_err());
    }

    #[test]
    fn parses_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/page");
                assert_eq!(result.title, "The Example Page");
                assert_eq!(result.content, "A snippet with markup.");
            }
            _ => panic!("expected a result"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&empty, &search()).unwrap().is_empty());
    }
}
