//! Pixabay engine, ported from `searx/engines/pixabay.py` (images and videos).
//!
//! Upstream scrapes the public search page rather than the documented API; the
//! `mediaType` of each entry decides whether an image or a video is emitted.
//! The `pixabay_type` setting (`images` or `videos`) is read from the
//! `search_type` config key.
// engine-id: pixabay

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "pix",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pixabay.com";

/// Pixabay, royalty free media.
pub struct Pixabay {
    meta: EngineMeta,
    search_type: String,
}

impl Pixabay {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let search_type = config
            .opt_str("search_type")
            .unwrap_or("images")
            .to_string();
        if !matches!(search_type.as_str(), "images" | "videos") {
            return Err(format!("pixabay: invalid search_type {search_type:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            search_type,
        }))
    }
}

impl Engine for Pixabay {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs = vec![("pagi", pageno.max(1).to_string())];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "1d",
                TimeRange::Week => "1w",
                TimeRange::Month => "1m",
                TimeRange::Year => "1y",
            };
            pairs.push(("date", value.to_string()));
        }
        // Upstream builds the path with `quote_plus`, so spaces become "+".
        let encoded_query = encode(&query.query_text()).replace("%20", "+");
        // Upstream's `safesearch_map`: {0: "off", 1: "1", 2: "1"}.
        let safe_search = if query.safesearch == 0 { "off" } else { "1" };
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/{}/search/{encoded_query}/?{}",
            self.search_type,
            query_string(&pairs)
        ))
        .header("Accept", "application/json")
        .header("x-bootstrap-cache-miss", "1")
        .header("x-fetch-bootstrap", "1")
        .cookie("g_rated", safe_search))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // Without an API key the engine stays silent instead of erroring.
        if self.meta.api_key.as_deref().unwrap_or_default().is_empty() {
            return Ok(Vec::new());
        }
        // A redirect means "no results on this page" or an auto-corrected query.
        if (300..400).contains(&response.status) {
            return Ok(Vec::new());
        }
        let json = response.json()?;
        let Some(items) = json_path(&json, "page.results").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let media_type = json_str(item, "mediaType");
            let href = json_str(item, "href");
            let title = json_str(item, "name");
            if title.is_empty() || href.is_empty() {
                continue;
            }
            let url = format!("{BASE_URL}{href}");
            match media_type.as_str() {
                "photo" | "illustration" | "vector" => {
                    let sources = ordered_sources(item);
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(json_str(item, "description"))
                            .image(sources.last().cloned().unwrap_or_default())
                            .thumbnail(sources.first().cloned().unwrap_or_default())
                            .template(Template::Images)
                            .build(),
                    );
                }
                "video" => {
                    let mut builder = ResultBuilder::new(url, title)
                        .content(json_str(item, "description"))
                        .thumbnail(json_str(item, "sources.thumbnail"))
                        .template(Template::Videos)
                        .metadata("iframe_src", json_str(item, "sources.embed"))
                        .metadata("length", json_str(item, "duration"));
                    let upload = json_str(item, "uploadDate");
                    if !upload.is_empty() {
                        builder = builder.published_date(first_ten(&upload));
                    }
                    results.push(builder.build());
                }
                _ => {}
            }
        }
        Ok(results)
    }
}

/// The `sources` values ordered by ascending image quality.
///
/// Upstream relies on the insertion order of Pixabay's JSON, in which the
/// quality levels are ordered ascending. This port ranks the known Pixabay
/// source names explicitly instead, which is order independent; unknown names
/// sort after the known ones.
fn ordered_sources(item: &serde_json::Value) -> Vec<String> {
    const QUALITY: &[&str] = &[
        "tiny",
        "small",
        "medium",
        "large",
        "largeImageURL",
        "fullHDURL",
        "imageURL",
        "vectorURL",
        "original",
    ];
    let Some(object) = json_path(item, "sources").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort_by_key(|key| {
        QUALITY
            .iter()
            .position(|name| name.eq_ignore_ascii_case(key))
            .unwrap_or(QUALITY.len())
    });
    keys.into_iter()
        .filter_map(|key| object.get(key).and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .filter(|url| !url.is_empty())
        .collect()
}

fn first_ten(value: &str) -> String {
    value.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "page": {"results": [
        {"mediaType": "photo", "href": "/photos/lake-1234/", "name": "Lake",
         "description": "A lake",
         "sources": {"tiny": "https://cdn.pixabay.com/tiny.jpg",
                     "small": "https://cdn.pixabay.com/small.jpg",
                     "large": "https://cdn.pixabay.com/large.jpg"}}
      ]}
    }"#;

    fn engine() -> Arc<Pixabay> {
        let mut config = EngineConfig::new("pixabay", "pixabay", "pix", &["images"]);
        config.api_key = Some("test-key".into());
        Pixabay::from_config(&config).unwrap()
    }

    #[test]
    fn parses_image_results() {
        let response = EngineResponse::new(
            200,
            "https://pixabay.com/images/search/lake/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Lake");
                assert_eq!(result.url, "https://pixabay.com/photos/lake-1234/");
                assert_eq!(result.template, Template::Images);
                // Upstream: thumbnail is the first (smallest) source, img_src
                // the last (largest) one.
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.pixabay.com/tiny.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://cdn.pixabay.com/large.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn returns_nothing_without_an_api_key() {
        let engine =
            Pixabay::from_config(&EngineConfig::new("pixabay", "pixabay", "pix", &["images"]))
                .unwrap();
        let response =
            EngineResponse::new(200, "https://pixabay.com/", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine.parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("lake house", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("/images/search/lake+house/?"));
        assert!(request.url.contains("pagi=2"));
        assert!(
            request
                .cookies
                .iter()
                .any(|(name, value)| name == "g_rated" && value == "off")
        );
    }
}
