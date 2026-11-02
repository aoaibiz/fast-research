//! Andisearch engine, ported from `searx/engines/andisearch.py` (web search).
//!
//! Upstream supports the categories `results`, `news` and `images` through the
//! `andisearch_categ` setting; this port implements the main `results` variant
//! and reads the category from the `search_type` config key.
//!
//! Upstream obtains a short lived API token from `GET /api/token` and caches it
//! for five minutes. The frozen `build_request` interface cannot perform that
//! extra round trip, so the token is read from the `api_token` config option.
// engine-id: andisearch

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "andi",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://andisearch.com";

/// Andisearch.
pub struct Andisearch {
    meta: EngineMeta,
    categ: String,
}

impl Andisearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("results")
            .to_string();
        if !matches!(categ.as_str(), "results" | "news" | "images") {
            return Err(format!("andisearch: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Andisearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = serde_json::json!({
            "inputTranscript": query.query_text(),
            "requestAttributes": {
                "compress": "false",
                "features": "deep answers",
                "persona": "mixed",
                "safety": "false",
                "cleanup": "true",
                "maxPayloadSize": "200000",
            },
            "sessionAttributes": {"format": "json"},
        });
        let mut request = EngineRequest::post(format!("{BASE_URL}/api/search"))
            .json_body(body)
            .header("Origin", BASE_URL)
            .header("Referer", &format!("{BASE_URL}/"))
            .header("Accept", "application/json");
        if let Some(token) = self.meta.opt_str("api_token") {
            request.set_header("x-api-token", token);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(raw) = json_path(&json, "sessionState.sessionAttributes.currentSearchResult")
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(Vec::new());
        };
        let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
            EngineError::Parse(format!("invalid currentSearchResult JSON: {error}"))
        })?;
        let mut results = Vec::new();
        if let Some(answer) = json_path(&parsed, "answer").and_then(serde_json::Value::as_str) {
            let answer = strip_tags(answer);
            if !answer.is_empty() {
                results.push(EngineItem::Answer(answer));
            }
        }
        let Some(items) = json_path(&parsed, &self.categ).and_then(serde_json::Value::as_array)
        else {
            return Ok(results);
        };
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "link");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            if self.categ == "images" {
                results.push(
                    ResultBuilder::new(url, title)
                        .image(json_str(item, "image"))
                        .template(crate::results::Template::Images)
                        .metadata("resolution", resolution(item))
                        .build(),
                );
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&json_str(item, "desc")))
                .thumbnail(json_str(item, "image"));
            if let Some(date) = json_path(item, "date").and_then(serde_json::Value::as_str) {
                builder = builder.published_date(take_first(date, 10));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn resolution(item: &serde_json::Value) -> String {
    let width = json_str(item, "width");
    let height = json_str(item, "height");
    if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width}x{height}")
    }
}

fn take_first(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "sessionState": {
        "sessionAttributes": {
          "currentSearchResult": "{\"answer\": \"Rust is a language\", \"results\": [{\"title\": \"Rust <b>lang</b>\", \"link\": \"https://www.rust-lang.org/\", \"desc\": \"Fast and safe\", \"image\": \"https://img.example.com/a.png\", \"date\": \"2024-04-01T10:00:00Z\"}]}"
        }
      }
    }"#;

    fn engine() -> Arc<Andisearch> {
        Andisearch::from_config(&EngineConfig::new(
            "andisearch",
            "andisearch",
            "andi",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_answer_and_results() {
        let response = EngineResponse::new(
            200,
            "https://andisearch.com/api/search",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Answer(answer) => assert_eq!(answer, "Rust is a language"),
            _ => panic!("expected an answer"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust lang");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "Fast and safe");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_post_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://andisearch.com/api/search");
        let body = request.json.expect("json body");
        assert_eq!(body["inputTranscript"], "rust");
    }
}
