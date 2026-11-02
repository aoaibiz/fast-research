//! Openverse engine, ported from `searx/engines/openverse.py`.

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
    shortcut: "ov",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.openverse.org/v1/images/";
/// Upstream requests 20 images per page.
const PAGE_SIZE: u32 = 20;

/// Openverse, the Creative Commons image search.
pub struct Openverse {
    meta: EngineMeta,
}

impl Openverse {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Openverse {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("page_size", PAGE_SIZE.to_string()),
            ("format", "json".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
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
            let title = json_str(item, "title");
            let url = json_str(item, "foreign_landing_url");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let image = json_str(item, "url");
            let thumbnail = if json_str(item, "thumbnail").is_empty() {
                image.clone()
            } else {
                json_str(item, "thumbnail")
            };
            let resolution = resolution(item);
            results.push(
                ResultBuilder::new(url, title)
                    .image(image)
                    .thumbnail(thumbnail)
                    .author(json_str(item, "creator"))
                    .published_date(json_str(item, "indexed_on"))
                    .template(Template::Images)
                    .metadata("resolution", resolution)
                    .metadata("license", json_str(item, "license"))
                    .metadata("license_version", json_str(item, "license_version"))
                    .metadata("source", json_str(item, "source"))
                    .metadata("width", json_str(item, "width"))
                    .metadata("height", json_str(item, "height"))
                    .metadata("filesize", json_str(item, "filesize"))
                    .metadata("type", json_str(item, "type"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The pixel size, like upstream's resolution field.
fn resolution(item: &serde_json::Value) -> String {
    let width = json_str(item, "width");
    let height = json_str(item, "height");
    if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width} x {height}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "result_count": 1,
      "page_count": 1,
      "results": [
        {
          "id": "abc123",
          "title": "Sunset over the sea",
          "url": "https://upload.wikimedia.org/wikipedia/commons/a/ab/Sunset.jpg",
          "thumbnail": "https://api.openverse.org/v1/images/abc123/thumb/",
          "foreign_landing_url": "https://commons.wikimedia.org/wiki/File:Sunset.jpg",
          "creator": "Jane Doe",
          "license": "by-sa",
          "license_version": "4.0",
          "source": "wikimedia",
          "width": 1024,
          "height": 768,
          "indexed_on": "2024-01-01T00:00:00Z"
        }
      ]
    }"#;

    fn engine() -> Arc<Openverse> {
        Openverse::from_config(&EngineConfig::new(
            "openverse",
            "openverse",
            "ov",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_image_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("sunset", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Sunset over the sea");
                assert_eq!(
                    result.url,
                    "https://commons.wikimedia.org/wiki/File:Sunset.jpg"
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://upload.wikimedia.org/wikipedia/commons/a/ab/Sunset.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://api.openverse.org/v1/images/abc123/thumb/")
                );
                assert_eq!(result.author.as_deref(), Some("Jane Doe"));
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("1024 x 768")
                );
                assert_eq!(
                    result.metadata.get("license").map(String::as_str),
                    Some("by-sa")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("sunset", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with(BASE_URL));
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("page_size=20"));
    }
}
