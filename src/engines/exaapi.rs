//! Exa Search API engine, ported from searx/engines/exaapi.py.
// engine-id: exaapi

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "exa",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.exa.ai/search";
const SEARCH_TYPES: &[&str] = &[
    "fast",
    "auto",
    "instant",
    "deep",
    "deep-lite",
    "deep-reasoning",
];
const CONTENT_MODES: &[&str] = &["highlights", "text"];
const DEFAULT_RESULTS: i64 = 10;
const DEFAULT_CHARACTERS: i64 = 500;

/// Exa, a search API for AI agents. Requires an api_key.
pub struct ExaApi {
    meta: EngineMeta,
}

fn iso_date(value: &str) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(value);
    date.map(|date| date.to_rfc3339()).unwrap_or_default()
}

impl ExaApi {
    /// The checks of the upstream setup() hook.
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let key = config
            .api_key
            .as_deref()
            .or_else(|| config.opt_str("api_key"));
        if key.map(str::trim).unwrap_or_default().is_empty() {
            return Err("exaapi requires an api_key".to_string());
        }
        let results = config
            .opt_i64("results_per_page")
            .unwrap_or(DEFAULT_RESULTS);
        let search_type = config.opt_str("search_type").unwrap_or("auto");
        let content_mode = config.opt_str("content_mode").unwrap_or("highlights");
        let characters = config
            .opt_i64("content_max_characters")
            .unwrap_or(DEFAULT_CHARACTERS);
        if !(1..=100).contains(&results)
            || !SEARCH_TYPES.contains(&search_type)
            || !CONTENT_MODES.contains(&content_mode)
            || characters < 1
        {
            return Err(format!(
                "invalid exaapi options: {search_type} {content_mode}"
            ));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn api_key(&self) -> Result<&str, EngineError> {
        let key = self
            .meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("api_key"));
        key.map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| EngineError::Message("exaapi requires an api_key".into()))
    }
}

impl Engine for ExaApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.api_key()?;
        let max_characters = self
            .meta
            .opt_i64("content_max_characters")
            .unwrap_or(DEFAULT_CHARACTERS)
            .max(1);
        let contents = match self.meta.opt_str("content_mode") {
            Some("text") => {
                serde_json::json!({"text": {"maxCharacters": max_characters, "stripLinks": true}})
            }
            _ => serde_json::json!({"highlights": {"maxCharacters": max_characters}}),
        };
        let mut body = serde_json::json!({
            "query": query.query_text(),
            "type": self.meta.opt_str("search_type").unwrap_or("auto"),
            "numResults": self.meta.opt_i64("results_per_page").unwrap_or(DEFAULT_RESULTS),
            "contents": contents,
        });
        if query.safesearch > 0 {
            body["moderation"] = serde_json::Value::Bool(true);
        }
        let region = locale_region(&query.locale);
        if region != "all" {
            body["userLocation"] = serde_json::Value::String(region.to_uppercase());
        }
        Ok(EngineRequest::post(BASE_URL)
            .header("x-api-key", key)
            .header("Accept", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let text_mode = self.meta.opt_str("content_mode") == Some("text");
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if !url.starts_with("https://") && !url.starts_with("http://") {
                continue;
            }
            let highlights: Vec<&str> = json_path(item, "highlights")
                .and_then(|value| value.as_array())
                .map(|values| values.iter().filter_map(|value| value.as_str()).collect())
                .unwrap_or_default();
            let excerpt = if text_mode {
                json_str(item, "text")
            } else {
                highlights.join(" ")
            };
            let title = strip_tags(&json_str(item, "title"));
            let title = if title.is_empty() { url.clone() } else { title };
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&excerpt))
                .author(json_str(item, "author"))
                .published_date(iso_date(&json_str(item, "publishedDate")));
            let image = json_str(item, "image");
            if image.starts_with("https://") || image.starts_with("http://") {
                builder = builder.thumbnail(image);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<ExaApi> {
        let mut config = EngineConfig::new("exaapi", "exaapi", "exa", &["general", "web"]);
        config.api_key = Some("KEY".to_string());
        ExaApi::from_config(&config).expect("exaapi engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_api_body() {
        let mut query = query("rust async");
        query.locale = "en-US".to_string();
        query.safesearch = 1;
        let request = engine().build_request(&query, 1).expect("request");
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["query"].as_str(), Some("rust async"));
        assert_eq!(body["userLocation"].as_str(), Some("US"));
        assert_eq!(
            body["contents"]["highlights"]["maxCharacters"].as_i64(),
            Some(500)
        );
    }

    #[test]
    fn parses_the_results() {
        let body = r##"{"results": [
          {"url": "https://exa.ai/", "title": "Exa <b>Search</b>", "highlights": ["A <i>search</i> engine.", "For agents."], "image": "https://exa.ai/og.png", "publishedDate": "2024-05-01T10:00:00Z"},
          {"title": "no url"}, {"url": "ftp://example.com/file", "title": "not http"}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("exa")).expect("items");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://exa.ai/");
                assert_eq!(result.title, "Exa Search");
                assert_eq!(result.content, "A search engine. For agents.");
                assert_eq!(result.thumbnail.as_deref(), Some("https://exa.ai/og.png"));
            }
            _ => panic!("expected a result"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine().parse(&empty, &query("exa"));
        assert!(items.expect("items").is_empty());
    }
}
