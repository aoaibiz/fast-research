//! TinEye engine, ported from searx/engines/tineye.py.
//!
//! Upstream is an online_url_search engine: it extracts the image URL from the
//! query before the request. The frozen interface only sees the parsed query,
//! so the data:image URI or the first http(s) token is used.
// engine-id: tineye

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "tin",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 9.0,
};

const BASE_URL: &str = "https://tineye.com";
const SEARCH_PATH: &str = "/api/v1/result_json/";

/// TinEye reverse image search by URL.
pub struct Tineye {
    meta: EngineMeta,
}

impl Tineye {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for Tineye {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let Some(image) = image_url(&query.query_text()) else {
            return Err(EngineError::Message(
                "tineye: the query must contain an image URL".into(),
            ));
        };
        let pairs: Vec<(&str, String)> = vec![("page", pageno.max(1).to_string()), ("url", image)];
        Ok(EngineRequest::get(format!(
            "{}{}?{}",
            self.base_url(),
            SEARCH_PATH,
            query_string(&pairs)
        ))
        .header("Connection", "keep-alive")
        .header("DNT", "1")
        .header("TE", "trailers"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // Upstream logs the suggestion text and returns nothing for these.
        if matches!(response.status, 400 | 422) {
            return Ok(Vec::new());
        }
        let json = response.json()?;
        let Some(entries) = json_path(&json, "matches").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let Some(backlink) = json_path(entry, "backlinks.0") else {
                continue;
            };
            let url = json_str(backlink, "backlink");
            let title = json_str(backlink, "image_name");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Images)
                .metadata("format", json_str(entry, "format"))
                .metadata("width", json_str(entry, "width"))
                .metadata("height", json_str(entry, "height"))
                .metadata("source", json_str(backlink, "url"))
                .published_date(json_str(backlink, "crawl_date"));
            // Upstream: img_src is the backlink image, the match image is the thumbnail.
            let backlink_image = json_str(backlink, "url");
            if is_http_url(&backlink_image) {
                builder = builder.image(backlink_image);
            }
            let thumbnail = json_str(entry, "image_url");
            if is_http_url(&thumbnail) {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The image URL of the query: a data:image URI wins over http(s), like
/// upstream's online_url_search processor.
fn image_url(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let data = parts.iter().find(|part| part.starts_with("data:image/"));
    let absolute = || {
        parts
            .iter()
            .find(|part| part.starts_with("http://") || part.starts_with("https://"))
    };
    data.or_else(absolute).map(|part| (*part).to_string())
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"matches":[{"image_url":"https://img.example/thumb.png","format":"PNG","width":800,"height":600,"backlinks":[{"url":"https://img.example/cats.png","backlink":"https://example.com/cats","image_name":"Cats","crawl_date":"2024-05-01"}]},{"image_url":"https://img.example/none.png","backlinks":[]}]}"##;

    fn engine() -> Arc<Tineye> {
        Tineye::from_config(&EngineConfig::new("tineye", "tineye", "tin", &["general"])).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_image_matches() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine()
            .parse(&response, &query("https://img.example/cats.png"))
            .unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/cats");
        assert_eq!(result.title, "Cats");
        assert_eq!(result.template, Template::Images);
        assert_eq!(
            (result.img_src.as_deref(), result.thumbnail.as_deref()),
            (
                Some("https://img.example/cats.png"),
                Some("https://img.example/thumb.png")
            )
        );
        assert_eq!(result.published_date.as_deref(), Some("2024-05-01"));
        assert_eq!(
            result.metadata.get("format").map(String::as_str),
            Some("PNG")
        );
    }

    #[test]
    fn client_errors_and_missing_matches_are_empty() {
        let body = br#"{"suggestions":{"key":"Download Error"}}"#;
        let response = EngineResponse::new(422, BASE_URL, body.to_vec());
        let items = engine()
            .parse(&response, &query("https://x.example/a.png"))
            .unwrap();
        assert!(items.is_empty());
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            engine()
                .parse(&response, &query("https://x.example/a.png"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn builds_the_request_from_the_image_url() {
        assert!(engine().build_request(&query("cats"), 1).is_err());
        let request = engine()
            .build_request(&query("https://example.com/cat.png"), 2)
            .unwrap();
        assert!(request.url.contains("page=2"));
        assert!(
            request
                .url
                .contains("url=https%3A%2F%2Fexample.com%2Fcat.png")
        );
    }
}
