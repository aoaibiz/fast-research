//! Neosearch engine, ported from searx/engines/neosearch.py.
//!
//! Upstream fetches the landing page first to read its xsrf-token meta tag;
//! this port reads the token from the xsrf_token engine option instead.
// engine-id: neosearch

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "neo",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://neosearch.org";

/// Neosearch, a privacy-first search engine.
pub struct Neosearch {
    meta: EngineMeta,
}

impl Neosearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Neosearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut body = serde_json::json!({"q": query.query_text(), "generate": "auto"});
        let region = locale_region(&query.locale).to_uppercase();
        if region.len() == 2 {
            if let Some(object) = body.as_object_mut() {
                object.insert("loc".to_string(), serde_json::Value::String(region));
            }
        }
        let mut request =
            EngineRequest::post(format!("{}/search", base.trim_end_matches('/'))).json_body(body);
        if let Some(token) = self.meta.opt_str("xsrf_token") {
            request = request.header("X-XSRF-TOKEN", token);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // The first line announces the location, the last one carries the results.
        let Some(line) = body.lines().rev().find(|line| !line.trim().is_empty()) else {
            return Ok(Vec::new());
        };
        let json: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| EngineError::Parse(format!("invalid response: {error}")))?;
        let mut items = Vec::new();
        if let Some(lenses) = json_path(&json, "lenses").and_then(|value| value.as_array()) {
            for lens in lenses {
                let categories = json_path(lens, "categories").and_then(|value| value.as_array());
                for category in categories.into_iter().flatten() {
                    let links = json_path(category, "links").and_then(|value| value.as_array());
                    for link in links.into_iter().flatten() {
                        let url = json_str(link, "url");
                        let title = json_str(link, "title");
                        if title.is_empty() || !is_http_url(&url) {
                            continue;
                        }
                        let mut content = json_str(link, "snippet");
                        if content.is_empty() {
                            content = json_str(link, "description");
                        }
                        items.push(ResultBuilder::new(url, title).content(content).build());
                    }
                }
            }
        }
        if let Some(suggestions) =
            json_path(&json, "suggestions").and_then(|value| value.as_array())
        {
            for suggestion in suggestions {
                if let Some(text) = suggestion.as_str() {
                    if !text.trim().is_empty() {
                        items.push(EngineItem::Suggestion(text.trim().to_string()));
                    }
                }
            }
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
    fn engine() -> Arc<Neosearch> {
        Neosearch::from_config(&EngineConfig::new(
            "neosearch",
            "neosearch",
            "neo",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_streamed_json() {
        let body = r##"{"location":"de"}
{"lenses":[{"categories":[{"links":[{"url":"https://example.com","title":"Example","snippet":"An example","description":"desc"}]}]}],"suggestions":["neosearch"]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com");
                assert_eq!(result.title, "Example");
                assert_eq!(result.content, "An example");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(value) => assert_eq!(value, "neosearch"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn builds_a_post_with_the_token_and_location() {
        let mut config = EngineConfig::new("neosearch", "neosearch", "neo", &["general"]);
        config.extra.insert(
            "xsrf_token".to_string(),
            toml::Value::String("token-1".to_string()),
        );
        let engine = Neosearch::from_config(&config).unwrap();
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "de-DE".to_string();
        let request = engine.build_request(&query, 1).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://neosearch.org/search");
        assert_eq!(
            request
                .headers
                .get("x-xsrf-token")
                .and_then(|value| value.to_str().ok()),
            Some("token-1")
        );
        let body = request.json.clone().unwrap();
        assert_eq!(body["q"], serde_json::json!("rust"));
        assert_eq!(body["loc"], serde_json::json!("DE"));
    }

    #[test]
    fn an_empty_body_yields_no_items() {
        let response = EngineResponse::new(200, BASE_URL, Vec::new());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
