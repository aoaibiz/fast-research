//! DuckDuckGo Extra engine, ported from `searx/engines/duckduckgo_extra.py`.
//!
//! Images, videos or news depending on the `ddg_category` engine option. The
//! upstream engine fetches a `vqd` token from duckduckgo.com and caches it; the
//! frozen `build_request` interface cannot do that round trip, so the token is
//! read from the `vqd` engine option.
// engine-id: duckduckgo_extra

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "videos", "news"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://duckduckgo.com";
const PATH_MAP: &[(&str, &str)] = &[("images", "i"), ("videos", "v"), ("news", "news")];

/// DuckDuckGo images/videos/news.
pub struct DuckDuckGoExtra {
    meta: EngineMeta,
    ddg_category: String,
}

impl DuckDuckGoExtra {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let ddg_category = config
            .extra
            .get("ddg_category")
            .and_then(toml::Value::as_str)
            .unwrap_or("images")
            .to_string();
        if !PATH_MAP.iter().any(|(name, _)| *name == ddg_category) {
            return Err(format!(
                "unsupported DuckDuckGo category {ddg_category:?} (expected images/videos/news)"
            ));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            ddg_category,
        }))
    }

    fn path(&self) -> &str {
        PATH_MAP
            .iter()
            .find(|(name, _)| *name == self.ddg_category)
            .map(|(_, path)| *path)
            .unwrap_or("i")
    }
}

impl Engine for DuckDuckGoExtra {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        if text.chars().count() >= 500 {
            return Err(EngineError::Message(
                "duckduckgo does not accept queries with 500 or more characters".into(),
            ));
        }
        let vqd = self.meta.opt_str("vqd").unwrap_or("").to_string();
        let region = locale_region(&query.locale);
        let region = if region.is_empty() || region == "all" {
            "wt-wt".to_string()
        } else {
            region
        };
        let lang = locale_language(&query.locale);
        let lang = if lang.is_empty() || lang == "all" {
            "en".to_string()
        } else {
            lang
        };
        let mut pairs = vec![
            ("o", "json".to_string()),
            ("q", text),
            ("u", "bing".to_string()),
            ("l", region.clone()),
            ("bpia", "1".to_string()),
            ("vqd", vqd),
            ("a", "h_".to_string()),
        ];
        let ct = if query.locale.is_empty() || query.locale == "all" {
            "EN".to_string()
        } else {
            query
                .locale
                .split('-')
                .next()
                .unwrap_or("EN")
                .to_uppercase()
        };
        pairs.push(("ct", ct));
        if pageno > 1 {
            pairs.push(("s", ((pageno - 1) * 100).to_string()));
        }
        // safesearch: 0 -> "1", 1 -> none, 2 -> "1"
        let safe = match query.safesearch {
            0 => Some("1"),
            1 => None,
            _ => Some("1"),
        };
        if let Some(safe) = safe {
            pairs.push(("p", safe.to_string()));
        }
        let mut request = EngineRequest::get(format!(
            "{BASE_URL}/{}.js?{}",
            self.path(),
            query_string(&pairs)
        ))
        .header("Accept", "*/*")
        .header("Referer", "https://duckduckgo.com/")
        .header("Host", "duckduckgo.com");
        request = request.cookie("ad", &lang);
        request = request.cookie("ah", &region);
        request = request.cookie("l", &region);
        let lang = if query.locale.is_empty() {
            String::new()
        } else {
            lang
        };
        request = request.header("Accept-Language", &lang);
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(results) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for result in results {
            let title = json_str(result, "title");
            if title.is_empty() {
                continue;
            }
            match self.ddg_category.as_str() {
                "images" => {
                    let url = json_str(result, "url");
                    let image = json_str(result, "image");
                    let thumbnail = json_str(result, "thumbnail");
                    if url.is_empty() || image.is_empty() {
                        continue;
                    }
                    items.push(
                        ResultBuilder::new(url, title)
                            .template(Template::Images)
                            .image(image)
                            .thumbnail(thumbnail)
                            .content(json_str(result, "resolution"))
                            .metadata("source", json_str(result, "source"))
                            .build(),
                    );
                }
                "videos" => {
                    let url = json_str(result, "content");
                    let thumbnail = json_path(result, "images.small")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            json_path(result, "images.medium")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                        .unwrap_or_default();
                    if url.is_empty() {
                        continue;
                    }
                    items.push(
                        ResultBuilder::new(url, title)
                            .template(Template::Videos)
                            .content(json_str(result, "description"))
                            .thumbnail(thumbnail)
                            .metadata("length", json_str(result, "duration"))
                            .metadata("uploader", json_str(result, "uploader"))
                            .build(),
                    );
                }
                _ => {
                    let url = json_str(result, "url");
                    if url.is_empty() {
                        continue;
                    }
                    let published = json_path(result, "date")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                        .map(|instant| instant.format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    items.push(
                        ResultBuilder::new(url, title)
                            .content(strip_tags(&json_str(result, "excerpt")))
                            .published_date(published)
                            .build(),
                    );
                }
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_news_results() {
        let fixture = r#"{
          "results": [
            {"url": "https://example.com/a", "title": "Rust News",
             "excerpt": "<b>Big</b> release", "date": 1704067200}
          ]
        }"#;
        let mut config = EngineConfig::new("ddg_news", "duckduckgo_extra", "", &["news"]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String("news".into()));
        let engine = DuckDuckGoExtra::from_config(&config).unwrap();
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.published_date.as_deref(), Some("2024-01-01"));
                assert_eq!(result.content, "Big release");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_image_results() {
        let fixture = r#"{
          "results": [
            {"url": "https://example.com/page", "title": "Logo",
             "image": "https://example.com/logo.png",
             "thumbnail": "https://example.com/thumb.png",
             "resolution": "800 x 600", "source": "example"}
          ]
        }"#;
        let mut config = EngineConfig::new("ddg_images", "duckduckgo_extra", "", &["images"]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String("images".into()));
        let engine = DuckDuckGoExtra::from_config(&config).unwrap();
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://example.com/logo.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn build_request_uses_the_category_path() {
        let mut config = EngineConfig::new("ddg_images", "duckduckgo_extra", "", &["images"]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String("images".into()));
        let engine = DuckDuckGoExtra::from_config(&config).unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://duckduckgo.com/i.js?"));
    }

    #[test]
    fn bad_category_is_rejected() {
        let mut config = EngineConfig::new("bad", "duckduckgo_extra", "", &["general"]);
        config
            .extra
            .insert("ddg_category".into(), toml::Value::String("maps".into()));
        assert!(DuckDuckGoExtra::from_config(&config).is_err());
    }
}
