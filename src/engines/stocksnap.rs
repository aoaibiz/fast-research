//! StockSnap engine, ported from searx/engines/stocksnap.py.
//!
//! StockSnap returns CC0 stock photos as JSON; the preview and the full size
//! URL are derived from the image id.
// engine-id: stocksnap

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["stock images"],
    shortcut: "sto",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://stocksnap.io";
const CDN_URL: &str = "https://cdn.stocksnap.io";

/// StockSnap, CC0 licensed stock photos.
pub struct Stocksnap {
    meta: EngineMeta,
}

impl Stocksnap {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for Stocksnap {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/api/search-photos/{}/relevance/desc/{}",
            self.base(),
            encode(&query.query_text()),
            pageno.max(1)
        )))
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
            let img_id = json_str(item, "img_id");
            let title = json_str(item, "tags");
            if img_id.is_empty() || title.is_empty() {
                continue;
            }
            let slug = format!("{}-{img_id}", first_keyword(item));
            let url = format!("{}/photo/{slug}", self.base());
            if !is_http(&url) {
                continue;
            }
            let thumbnail = format!("{CDN_URL}/img-thumbs/280h/{img_id}.jpg");
            let img_src = format!("{CDN_URL}/img-thumbs/960w/{img_id}.jpg");
            let resolution = format!(
                "{}x{}",
                json_str(item, "img_width"),
                json_str(item, "img_height")
            );
            results.push(
                ResultBuilder::new(url, title)
                    .thumbnail(thumbnail)
                    .image(img_src)
                    .template(Template::Images)
                    .metadata("img_format", "JPEG")
                    .metadata("resolution", resolution)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream takes the first keyword for the photo slug.
fn first_keyword(item: &serde_json::Value) -> String {
    json_path(item, "keywords")
        .and_then(serde_json::Value::as_array)
        .and_then(|values| values.first())
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"results":[
      {"img_id":"abc123","tags":"Mountain lake","keywords":["mountain","lake"],"img_width":4000,"img_height":3000},
      {"img_id":"","tags":"No id","keywords":["x"],"img_width":100,"img_height":100},
      {"img_id":"noTags","tags":"","keywords":["y"],"img_width":100,"img_height":100}
    ]}"##;

    fn engine() -> Arc<Stocksnap> {
        Stocksnap::from_config(&EngineConfig::new(
            "stocksnap",
            "stocksnap",
            "sto",
            &["stock images"],
        ))
        .unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_photos() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("mountain")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://stocksnap.io/photo/mountain-abc123");
                assert_eq!(result.title, "Mountain lake");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.stocksnap.io/img-thumbs/280h/abc123.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://cdn.stocksnap.io/img-thumbs/960w/abc123.jpg")
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.metadata.get("img_format").map(String::as_str),
                    Some("JPEG")
                );
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("4000x3000")
                );
            }
            _ => panic!("expected a photo result"),
        }
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&query("mountain lake"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://stocksnap.io/api/search-photos/mountain%20lake/relevance/desc/2"
        );
    }

    #[test]
    fn missing_results_yield_no_items() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query("x")).unwrap().is_empty());
    }
}
