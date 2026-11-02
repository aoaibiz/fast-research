//! Pexels engine, ported from `searx/engines/pexels.py` (stock images).
//!
//! Upstream extracts a short lived `secret-key` from Pexels' JavaScript bundles
//! (with a hardcoded fallback key). The frozen `build_request` interface cannot
//! perform those extra round trips, so the key is read from the `api_key`
//! config option; the engine returns an empty result list when it is missing.
// engine-id: pexels

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["stock images"],
    shortcut: "px",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.pexels.com";
const RESULTS_PER_PAGE: i64 = 20;

/// Pexels.
pub struct Pexels {
    meta: EngineMeta,
    results_per_page: i64,
}

impl Pexels {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            results_per_page: config
                .opt_i64("results_per_page")
                .unwrap_or(RESULTS_PER_PAGE),
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Pexels {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs = vec![
            ("query", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("per_page", self.results_per_page.to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "last_24_hours",
                TimeRange::Week => "last_week",
                TimeRange::Month => "last_month",
                TimeRange::Year => "last_year",
            };
            pairs.push(("date_from", value.to_string()));
        }
        let mut request = EngineRequest::get(format!(
            "{BASE_URL}/en-us/api/v3/search/photos?{}",
            query_string(&pairs)
        ))
        .header("Accept", "application/json");
        if let Some(key) = self.meta.api_key.as_deref().filter(|key| !key.is_empty()) {
            request.set_header("secret-key", key);
        }
        Ok(request)
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
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let attributes = json_path(item, "attributes").unwrap_or(item);
            let title = json_str(attributes, "title");
            let slug = json_str(attributes, "slug");
            let id = json_str(attributes, "id");
            if title.is_empty() || slug.is_empty() || id.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(format!("{BASE_URL}/photo/{slug}-{id}/"), title)
                    .content(json_str(attributes, "description"))
                    .author(json_str(attributes, "user.username"))
                    .image(json_str(attributes, "image.download_link"))
                    .thumbnail(json_str(attributes, "image.small"))
                    .template(Template::Images)
                    .metadata("resolution", resolution(attributes))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn resolution(item: &serde_json::Value) -> String {
    let width = json_str(item, "width");
    let height = json_str(item, "height");
    if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width}x{height}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": [
        {"attributes": {
          "id": 42, "slug": "mountain-lake", "title": "Mountain lake",
          "description": "A lake", "width": 4000, "height": 3000,
          "image": {"small": "https://images.pexels.com/small.jpg",
                    "download_link": "https://images.pexels.com/full.jpg"},
          "user": {"username": "jane"}
        }}
      ]
    }"#;

    fn engine() -> Arc<Pexels> {
        let mut config = EngineConfig::new("pexels", "pexels", "px", &["stock images"]);
        config.api_key = Some("test-key".into());
        Pexels::from_config(&config).unwrap()
    }

    #[test]
    fn parses_image_results() {
        let response = EngineResponse::new(
            200,
            "https://www.pexels.com/en-us/api/v3/search/photos",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Mountain lake");
                assert_eq!(result.url, "https://www.pexels.com/photo/mountain-lake-42/");
                assert_eq!(result.author.as_deref(), Some("jane"));
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn returns_nothing_without_an_api_key() {
        let engine = Pexels::from_config(&EngineConfig::new(
            "pexels",
            "pexels",
            "px",
            &["stock images"],
        ))
        .unwrap();
        let response =
            EngineResponse::new(200, "https://www.pexels.com/", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine.parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url_with_the_time_range() {
        let mut query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("per_page=20"));
        assert!(request.url.contains("date_from=last_week"));
        assert_eq!(
            request
                .headers
                .get("secret-key")
                .map(|value| value.to_str().unwrap()),
            Some("test-key")
        );
    }
}
