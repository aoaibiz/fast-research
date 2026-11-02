//! Dogpile engine, ported from `searx/engines/dogpile.py` (web search).
//!
//! Upstream picks the endpoint from `dogpile_categ` (`search`, `images`,
//! `videos`, `news`); this port implements the main `search` variant and reads
//! the category from the `search_type` config key.
//!
//! Upstream obtains a short lived API token from `/api/token/refresh` before
//! every request. The frozen `build_request` interface cannot perform that
//! extra round trip, so the token is read from the `token` config option.
// engine-id: dogpile

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "dp",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.dogpile.com";

/// Dogpile, a metasearch engine by System1.
pub struct Dogpile {
    meta: EngineMeta,
    categ: String,
}

impl Dogpile {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("search")
            .to_string();
        if !matches!(categ.as_str(), "search" | "images" | "videos" | "news") {
            return Err(format!("dogpile: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Dogpile {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let safe_search = match query.safesearch {
            0 => "none",
            1 => "moderate",
            _ => "heavy",
        };
        let body = serde_json::json!({
            "q": query.query_text(),
            "qadf": safe_search,
            "page": pageno.max(1),
        });
        let mut request = EngineRequest::post(format!("{BASE_URL}/api/{}", self.categ))
            .json_body(body)
            .header("Origin", BASE_URL)
            .header("Sec-Fetch-Mode", "cors")
            .cookie("dp_api_token", "1");
        if let Some(token) = self.meta.opt_str("token") {
            request.set_header("x-dogpile-token", token);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "clickUrl");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let content = strip_tags(&json_str(item, "description"));
            results.push(ResultBuilder::new(url, title).content(content).build());
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

    const FIXTURE: &str = r#"{
      "results": [
        {"title": "Rust &amp; Cargo", "clickUrl": "https://example.com/rust",
         "description": "<b>Fast</b> and reliable"}
      ]
    }"#;

    fn engine() -> Arc<Dogpile> {
        Dogpile::from_config(&EngineConfig::new("dogpile", "dogpile", "dp", &["general"])).unwrap()
    }

    #[test]
    fn parses_search_results() {
        let response = EngineResponse::new(
            200,
            "https://www.dogpile.com/api/search",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust & Cargo");
                assert_eq!(result.url, "https://example.com/rust");
                assert_eq!(result.content, "Fast and reliable");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_post_request_with_the_safesearch_setting() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.safesearch = 2;
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://www.dogpile.com/api/search");
        let body = request.json.expect("json body");
        assert_eq!(body["q"], "rust");
        assert_eq!(body["qadf"], "heavy");
        assert_eq!(body["page"], 2);
    }
}
