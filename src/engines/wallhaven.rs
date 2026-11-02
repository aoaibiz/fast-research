//! Wallhaven engine, ported from `searx/engines/wallhaven.py`.
//!
//! NSFW purities require a valid API key: without one only the "sfw" purity is
//! accepted by the API.  The key is read from `api_key`.
// engine-id: wallhaven

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "wh",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://wallhaven.cc";

/// Wallhaven.
pub struct Wallhaven {
    meta: EngineMeta,
}

impl Wallhaven {
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

    fn api_key(&self) -> Option<String> {
        self.meta
            .api_key
            .clone()
            .or_else(|| self.meta.opt_str("api_key").map(str::to_string))
            .filter(|key| !key.trim().is_empty())
    }
}

impl Engine for Wallhaven {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("purity", purity(query.safesearch).to_string()),
        ];
        let mut request = EngineRequest::get(format!(
            "{}/api/v1/search?{}",
            self.base_url(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json");
        if let Some(api_key) = self.api_key() {
            request = request.header("X-API-Key", &api_key);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if !is_http_url(&url) {
                continue;
            }
            // Wallhaven wallpapers have no title; upstream leaves it empty, here
            // the id keeps the result usable.
            let title = {
                let title = json_str(item, "id");
                if title.is_empty() {
                    "Wallhaven".to_string()
                } else {
                    title
                }
            };
            let filesize = json_path(item, "file_size")
                .and_then(serde_json::Value::as_f64)
                .map(humanize_bytes)
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(format!(
                        "{} / {}",
                        json_str(item, "category"),
                        json_str(item, "purity")
                    ))
                    .image(json_str(item, "path"))
                    .thumbnail(json_str(item, "thumbs.small"))
                    .published_date(json_str(item, "created_at"))
                    .template(Template::Images)
                    .metadata(
                        "resolution",
                        json_str(item, "resolution").replace('x', " x "),
                    )
                    .metadata("img_format", json_str(item, "file_type"))
                    .metadata("filesize", filesize)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream `safesearch_map`.
fn purity(safesearch: u8) -> &'static str {
    match safesearch {
        0 => "111",
        1 => "110",
        _ => "100",
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream's `humanize_bytes` (base 1024, two decimals).
fn humanize_bytes(mut size: f64) -> String {
    const SUFFIX: [&str; 5] = ["B ", "KB", "MB", "GB", "TB"];
    let mut position = 0usize;
    while size > 1024.0 && position < SUFFIX.len() {
        position += 1;
        size /= 1024.0;
    }
    let position = position.min(SUFFIX.len() - 1);
    format!("{size:.2} {}", SUFFIX[position])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": [
        {
          "id": "abc123",
          "url": "https://wallhaven.cc/w/abc123",
          "category": "anime",
          "purity": "sfw",
          "resolution": "1920x1080",
          "file_type": "image/jpeg",
          "file_size": 1572864,
          "path": "https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg",
          "thumbs": {"small": "https://th.wallhaven.cc/small/ab/abc123.jpg"},
          "created_at": "2024-01-01T00:00:00Z"
        }
      ]
    }"#;

    fn engine() -> Arc<Wallhaven> {
        Wallhaven::from_config(&EngineConfig::new(
            "wallhaven",
            "wallhaven",
            "wh",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_wallpapers() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("anime", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "abc123");
                assert_eq!(result.url, "https://wallhaven.cc/w/abc123");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg")
                );
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("1920 x 1080")
                );
                assert_eq!(
                    result.metadata.get("filesize").map(String::as_str),
                    Some("1.50 MB")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let mut query = Query::parse("anime", &crate::engine::EngineRegistry::default(), &[]);
        query.safesearch = 2;
        let request = engine().build_request(&query, 3).unwrap();
        assert_eq!(
            request.url,
            "https://wallhaven.cc/api/v1/search?q=anime&page=3&purity=100"
        );
        assert!(request.headers.get("X-API-Key").is_none());
    }
}
