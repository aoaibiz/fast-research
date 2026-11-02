//! Tootfinder engine, ported from `searx/engines/tootfinder.py`.
//!
//! The API sometimes appends an HTML error page to the JSON payload, so like
//! upstream only the line starting with `[{` is parsed.
// engine-id: tootfinder

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "toot",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.tootfinder.ch";

/// Tootfinder.
pub struct Tootfinder {
    meta: EngineMeta,
}

impl Tootfinder {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Tootfinder {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // Unlike upstream the query is percent-encoded: it is a path segment.
        Ok(EngineRequest::get(format!(
            "{}/rest/api/search/{}",
            self.base_url(),
            encode(&query.query_text())
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let Some(payload) = body
            .lines()
            .find(|line| line.trim_start().starts_with("[{"))
        else {
            return Ok(Vec::new());
        };
        let json: serde_json::Value = serde_json::from_str(payload.trim())
            .map_err(|error| EngineError::Parse(format!("invalid tootfinder response: {error}")))?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let content = strip_tags(&json_str(item, "content"));
            let card_title = json_str(item, "card.title");
            let title = if card_title.is_empty() {
                content.chars().take(75).collect::<String>()
            } else {
                card_title
            };
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let thumbnail = json_path(item, "media_attachments")
                .and_then(serde_json::Value::as_array)
                .and_then(|attachments| {
                    attachments.iter().find(|attachment| {
                        json_str(attachment, "type") == "image"
                            && !json_str(attachment, "preview_url").is_empty()
                    })
                })
                .map(|attachment| json_str(attachment, "preview_url"))
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(content)
                    .thumbnail(thumbnail)
                    .published_date(json_str(item, "created_at"))
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

    const FIXTURE: &str = "some server side error notice\n[{\"url\": \"https://mastodon.social/@user/1\", \"content\": \"<p>Hello <b>world</b></p>\", \"created_at\": \"2024-05-01T12:00:00.000Z\", \"card\": {\"title\": \"A card title\"}, \"media_attachments\": [{\"type\": \"image\", \"preview_url\": \"https://files.example/pic.png\"}]}]\ntrailing garbage";

    fn engine() -> Arc<Tootfinder> {
        Tootfinder::from_config(&EngineConfig::new(
            "tootfinder",
            "tootfinder",
            "toot",
            &["social media"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_json_line_only() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("hello", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A card title");
                assert_eq!(result.url, "https://mastodon.social/@user/1");
                assert_eq!(result.content, "Hello world");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://files.example/pic.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn error_pages_yield_no_results() {
        let response = EngineResponse::new(
            200,
            BASE_URL,
            b"<html>internal server error</html>".to_vec(),
        );
        let query = Query::parse("hello", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_search_url() {
        let query = Query::parse(
            "hello world",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://www.tootfinder.ch/rest/api/search/hello%20world"
        );
    }
}
