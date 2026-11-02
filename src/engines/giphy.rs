//! Giphy engine, ported from `searx/engines/giphy.py`.
//!
//! Upstream extracts an API key from Giphy's JavaScript bundles.  That is a
//! network round trip, which `build_request` cannot make, so the key is read
//! from `api_key`; without one the engine is disabled.
// engine-id: giphy

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
    shortcut: "gif",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.giphy.com";
const PAGE_SIZE: u32 = 15;

/// Giphy.
pub struct Giphy {
    meta: EngineMeta,
}

impl Giphy {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let engine = Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        };
        match engine.category() {
            "gifs" | "stickers" | "clips" => Ok(Arc::new(engine)),
            other => Err(format!(
                "giphy: giphy_categ must be gifs, stickers or clips (got {other:?})"
            )),
        }
    }

    fn api_url(&self) -> String {
        self.meta
            .opt_str("api_url")
            .unwrap_or(API_URL)
            .trim_end_matches('/')
            .to_string()
    }

    /// The search category (`gifs`, `stickers` or `clips`).
    fn category(&self) -> &str {
        self.meta.opt_str("giphy_categ").unwrap_or("gifs")
    }

    fn api_key(&self) -> Option<String> {
        self.meta
            .api_key
            .clone()
            .or_else(|| self.meta.opt_str("api_key").map(str::to_string))
            .filter(|key| !key.trim().is_empty())
    }
}

impl Engine for Giphy {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let Some(api_key) = self.api_key() else {
            return Err(EngineError::Message("giphy: an api_key is required".into()));
        };
        let category = self.category();
        let page_size = self
            .meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(PAGE_SIZE as i64) as u32;
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("api_key", api_key),
            ("limit", page_size.to_string()),
            ("offset", ((pageno.max(1) - 1) * page_size).to_string()),
            ("type", category.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/v1/{category}/search?{}",
            self.api_url(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
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
            let title = json_str(item, "title");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let img_src = json_str(item, "images.original.url");
            let thumbnail = [
                "images.downsized.url",
                "images.downsized_medium.url",
                "images.downsized_small.url",
                "images.downsized_large.url",
            ]
            .iter()
            .map(|path| json_str(item, path))
            .find(|value| !value.is_empty())
            .unwrap_or_else(|| img_src.clone());
            let resolution = format!(
                "{}x{}",
                json_str(item, "images.original.width"),
                json_str(item, "images.original.height")
            );
            let filesize = json_path(item, "images.original.size")
                .and_then(serde_json::Value::as_f64)
                .map(humanize_bytes)
                .unwrap_or_default();
            let tags = json_path(item, "tags")
                .and_then(serde_json::Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(tags)
                    .author(json_str(item, "username"))
                    .image(img_src)
                    .thumbnail(thumbnail)
                    .template(Template::Images)
                    .metadata("resolution", resolution)
                    .metadata("img_format", "GIF")
                    .metadata("filesize", filesize)
                    .metadata("source", json_str(item, "source_tld"))
                    .build(),
            );
        }
        Ok(results)
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
          "title": "A gif",
          "url": "https://giphy.com/gifs/a-gif-abc123",
          "username": "giphy",
          "source_tld": "example.com",
          "tags": ["cat", "funny"],
          "images": {
            "original": {
              "url": "https://media.giphy.com/media/abc123/giphy.gif",
              "width": "480",
              "height": "270",
              "size": "1572864"
            },
            "downsized": {"url": "https://media.giphy.com/media/abc123/downsized.gif"}
          }
        }
      ]
    }"#;

    fn engine() -> Arc<Giphy> {
        let mut config = EngineConfig::new("giphy", "giphy", "gif", &["images"]);
        config.extra.insert(
            "api_key".to_string(),
            toml::Value::String("dc6zaTOxFJmzC".to_string()),
        );
        Giphy::from_config(&config).unwrap()
    }

    #[test]
    fn parses_gifs() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A gif");
                assert_eq!(result.url, "https://giphy.com/gifs/a-gif-abc123");
                assert_eq!(result.content, "cat, funny");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://media.giphy.com/media/abc123/giphy.gif")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://media.giphy.com/media/abc123/downsized.gif")
                );
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("480x270")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.giphy.com/v1/gifs/search?")
        );
        assert!(request.url.contains("api_key=dc6zaTOxFJmzC"));
        assert!(request.url.contains("offset=15"));
        assert!(request.url.contains("type=gifs"));
    }

    #[test]
    fn without_an_api_key_the_engine_is_disabled() {
        let config = EngineConfig::new("giphy", "giphy", "gif", &["images"]);
        let engine = Giphy::from_config(&config).unwrap();
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine.build_request(&query, 1),
            Err(EngineError::Message(_))
        ));
    }
}
