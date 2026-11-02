//! Unsplash engine, ported from `searx/engines/unsplash.py` (stock images).
//!
//! Upstream's Unsplash endpoint is behind Anubis, which blocks the common user
//! agents; upstream therefore sends the SearXNG user agent, which is not
//! commonly used by crawlers. This port keeps the plain request and documents
//! the limitation.
// engine-id: unsplash

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
    shortcut: "us",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://unsplash.com/napi/search/photos?";
const PAGE_SIZE: i64 = 20;

/// Unsplash.
pub struct Unsplash {
    meta: EngineMeta,
    page_size: i64,
}

impl Unsplash {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            page_size: config.opt_i64("page_size").unwrap_or(PAGE_SIZE),
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Unsplash {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("query", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("per_page", self.page_size.to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
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
        let Some(items) = json_path(&json, "results").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = clean_url(&json_str(item, "links.html"));
            if !is_http_url(&url) {
                continue;
            }
            let mut title = json_str(item, "alt_description");
            if title.is_empty() {
                title = "unknown".into();
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(json_str(item, "description"))
                    .image(clean_url(&json_str(item, "urls.regular")))
                    .thumbnail(clean_url(&json_str(item, "urls.thumb")))
                    .template(Template::Images)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Drop Unsplash's `ixid` tracking parameter.
fn clean_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| key != "ixid")
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        let query = kept
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&query));
    }
    parsed.to_string()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "results": [
        {"alt_description": "A mountain", "description": "Lake and mountain",
         "links": {"html": "https://unsplash.com/photos/abc?ixid=XYZ&utm_source=x"},
         "urls": {"thumb": "https://images.unsplash.com/thumb?ixid=XYZ",
                  "regular": "https://images.unsplash.com/regular?ixid=XYZ"}}
      ]
    }"#;

    fn engine() -> Arc<Unsplash> {
        let mut config = EngineConfig::new("unsplash", "unsplash", "us", &["stock images"]);
        config.api_key = Some("test-key".into());
        Unsplash::from_config(&config).unwrap()
    }

    #[test]
    fn parses_image_results_and_drops_ixid() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("mountain", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A mountain");
                assert_eq!(result.url, "https://unsplash.com/photos/abc?utm_source=x");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://images.unsplash.com/regular")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn returns_nothing_without_an_api_key() {
        let engine = Unsplash::from_config(&EngineConfig::new(
            "unsplash",
            "unsplash",
            "us",
            &["stock images"],
        ))
        .unwrap();
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("mountain", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine.parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("mountain", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with(SEARCH_URL));
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("per_page=20"));
    }
}
