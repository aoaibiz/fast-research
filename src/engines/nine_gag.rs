//! 9GAG engine, ported from `searx/engines/9gag.py` (social media).
// engine-id: 9gag

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "9g",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://9gag.com/v1/search-posts";
const PAGE_SIZE: i64 = 10;

/// 9GAG.
pub struct NineGag {
    meta: EngineMeta,
    page_size: i64,
}

impl NineGag {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            page_size: config.opt_i64("page_size").unwrap_or(PAGE_SIZE),
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for NineGag {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("query", query.query_text()),
            (
                "c",
                (((pageno.max(1) - 1) as i64) * self.page_size).to_string(),
            ),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let mut results = Vec::new();
        if let Some(posts) = json_path(&json, "data.posts").and_then(serde_json::Value::as_array) {
            for post in posts {
                let title = json_str(post, "title");
                let url = json_str(post, "url");
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                // Use the not cropped thumbnail when the image height matters.
                let height = json_path(post, "images.image700.height")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                let thumbnail = if height > 400 {
                    json_str(post, "images.imageFbThumbnail.url")
                } else {
                    json_str(post, "images.image700.url")
                };
                let content = json_str(post, "description");
                let mut builder = ResultBuilder::new(url, title)
                    .content(content)
                    .thumbnail(thumbnail);
                match json_str(post, "type").as_str() {
                    "Photo" => {
                        builder = builder
                            .image(json_str(post, "images.image700.url"))
                            .template(Template::Images);
                    }
                    "Animated" => {
                        builder = builder
                            .metadata("iframe_src", json_str(post, "images.image460sv.url"))
                            .template(Template::Videos);
                    }
                    _ => continue,
                }
                if let Some(timestamp) =
                    json_path(post, "creationTs").and_then(serde_json::Value::as_i64)
                {
                    if let Some(instant) = chrono::DateTime::from_timestamp(timestamp, 0) {
                        builder = builder.published_date(instant.format("%Y-%m-%d").to_string());
                    }
                }
                results.push(builder.build());
            }
        }
        if let Some(tags) = json_path(&json, "data.tags").and_then(serde_json::Value::as_array) {
            for tag in tags {
                let key = json_str(tag, "key");
                if !key.is_empty() {
                    results.push(EngineItem::Suggestion(key));
                }
            }
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

    const FIXTURE: &str = r#"{
      "data": {
        "posts": [
          {"type": "Photo", "url": "https://9gag.com/gag/a1", "title": "A meme",
           "description": "desc", "creationTs": 1700000000,
           "images": {"image700": {"url": "https://img.9gag.com/700.jpg", "height": 300},
                      "imageFbThumbnail": {"url": "https://img.9gag.com/fb.jpg"}}}
        ],
        "tags": [{"key": "memes"}]
      }
    }"#;

    fn engine() -> Arc<NineGag> {
        NineGag::from_config(&EngineConfig::new("9gag", "9gag", "9g", &["social media"])).unwrap()
    }

    #[test]
    fn parses_photo_posts_and_tags() {
        let response = EngineResponse::new(
            200,
            "https://9gag.com/v1/search-posts",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("meme", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A meme");
                assert_eq!(result.url, "https://9gag.com/gag/a1");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.9gag.com/700.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.9gag.com/700.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "memes"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("meme", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with("https://9gag.com/v1/search-posts?"));
        assert!(request.url.contains("c=20"));
    }
}
