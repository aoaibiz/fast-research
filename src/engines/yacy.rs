//! Yacy engine, ported from searx/engines/yacy.py.
//!
//! Upstream supports the text and image search types of a local or global YaCy
//! peer. The digest authentication options are not ported: EngineRequest only
//! carries headers and cookies.
// engine-id: yacy

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
    shortcut: "ya",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// Upstream page_size.
const PAGE_SIZE: i64 = 10;

/// A YaCy peer, searched through its JSON API.
pub struct Yacy {
    meta: EngineMeta,
}

impl Yacy {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let search_type = meta.opt_str("search_type").unwrap_or("text");
        if search_type != "text" && search_type != "image" {
            return Err(format!(
                "yacy: search_type {search_type:?} is not text or image"
            ));
        }
        Ok(Arc::new(Self { meta }))
    }

    fn search_type(&self) -> &str {
        self.meta.opt_str("search_type").unwrap_or("text")
    }

    /// The peer URL: base_url may be a string or a list of strings.
    fn base_url(&self) -> Option<String> {
        self.meta
            .base_url
            .clone()
            .or_else(|| self.meta.opt_str_list("base_url").into_iter().next())
            .map(|url| url.trim().trim_end_matches('/').to_string())
            .filter(|url| !url.is_empty())
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream parses pubDate with dateutil and serialises ISO 8601.
fn iso_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value.trim())
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

impl Engine for Yacy {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .base_url()
            .ok_or_else(|| EngineError::Message("yacy requires a base_url option".to_string()))?;
        let page_size = self.meta.opt_i64("page_size").unwrap_or(PAGE_SIZE).max(1);
        let offset = (i64::from(pageno.max(1)) - 1) * page_size;
        let mut pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("startRecord", offset.to_string()),
            ("maximumRecords", page_size.to_string()),
            ("contentdom", self.search_type().to_string()),
            (
                "resource",
                self.meta
                    .opt_str("search_mode")
                    .unwrap_or("global")
                    .to_string(),
            ),
        ];
        let language = query.engine_language();
        if language != "all" {
            let code = language
                .split('-')
                .next()
                .unwrap_or_default()
                .to_lowercase();
            pairs.push(("lr", format!("lang_{code}")));
        }
        let url = format!("{base}/yacysearch.json?{}", query_string(&pairs));
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = json_path(&json, "channels.0.items").and_then(serde_json::Value::as_array);
        let Some(items) = items else {
            return Ok(Vec::new());
        };
        let images = self.search_type() == "image";
        let mut results = Vec::new();
        for item in items {
            if images {
                let url = json_str(item, "url");
                let url = if url.is_empty() {
                    json_str(item, "link")
                } else {
                    url
                };
                let title = json_str(item, "title");
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(url, title)
                        .template(Template::Images)
                        .image(json_str(item, "image"))
                        .build(),
                );
                continue;
            }
            let url = json_str(item, "link");
            let title = json_str(item, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(item, "description")))
                    .published_date(iso_date(&json_str(item, "pubDate")))
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "channels": [{
        "items": [
          {
            "link": "https://example.com/a",
            "title": "Example A",
            "description": "<b>hello</b> world",
            "pubDate": "2024-05-01T12:00:00Z"
          },
          {"title": "No link", "description": "ignored"},
          {"link": "ftp://example.com/b", "title": "Not http"}
        ]
      }]
    }"##;

    fn engine(search_type: &str) -> Arc<Yacy> {
        let mut config = EngineConfig::new("yacy", "yacy", "ya", &["general"]);
        config.base_url = Some("https://yacy.example/".to_string());
        config.extra.insert(
            "search_type".to_string(),
            toml::Value::String(search_type.to_string()),
        );
        Yacy::from_config(&config).expect("yacy engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_text_results() {
        let response =
            EngineResponse::new(200, "https://yacy.example", FIXTURE.as_bytes().to_vec());
        let items = engine("text").parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/a");
                assert_eq!(result.title, "Example A");
                assert_eq!(result.content, "hello world");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:00:00+00:00")
                );
            }
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn builds_the_paged_json_url() {
        let mut query = query("rust");
        query.locale = "ja-JP".to_string();
        let request = engine("text").build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://yacy.example/yacysearch.json?query=rust&startRecord=10&maximumRecords=10&contentdom=text&resource=global&lr=lang_ja"
        );
    }

    #[test]
    fn image_results_use_the_images_template() {
        let body = r##"{"channels": [{"items": [{"url": "https://img.example.com/a.jpg", "title": "A", "image": "https://img.example.com/a.jpg"}]}]}"##;
        let response = EngineResponse::new(200, "https://yacy.example", body.as_bytes().to_vec());
        let items = engine("image").parse(&response, &query("cat")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.example.com/a.jpg")
                );
            }
            other => panic!("expected a result, got {other:?}"),
        }
        let mut config = EngineConfig::new("yacy", "yacy", "ya", &["general"]);
        config.base_url = Some("https://yacy.example".to_string());
        config.extra.insert(
            "search_type".to_string(),
            toml::Value::String("video".to_string()),
        );
        assert!(Yacy::from_config(&config).is_err());
    }
}
