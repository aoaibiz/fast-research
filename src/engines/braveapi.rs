//! Brave Search API engine, ported from `searx/engines/braveapi.py`.

// engine-id: braveapi

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "braveapi",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const DEFAULT_RESULTS_PER_PAGE: u32 = 20;

/// Brave Search API. Requires an API key.
pub struct BraveApi {
    meta: EngineMeta,
}

impl BraveApi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn results_per_page(&self) -> u32 {
        self.meta
            .opt_i64("results_per_page")
            .filter(|value| *value > 0)
            .map(|value| value as u32)
            .unwrap_or(DEFAULT_RESULTS_PER_PAGE)
    }
}

impl Engine for BraveApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("count", self.results_per_page().to_string()),
            ("offset", pageno.saturating_sub(1).to_string()),
            ("text_decorations", "false".to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "past_day",
                TimeRange::Week => "past_week",
                TimeRange::Month => "past_month",
                TimeRange::Year => "past_year",
            };
            pairs.push(("time_range", value.to_string()));
        }
        pairs.push((
            "safesearch",
            match query.safesearch {
                2 => "strict",
                1 => "moderate",
                _ => "off",
            }
            .to_string(),
        ));
        let key = self.meta.api_key.as_deref().unwrap_or_default();
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("X-Subscription-Token", key)
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "web.results").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            // Mirror the shared parse_video_result of the Brave engine: the
            // title, url and description are the web result fields.
            let title = json_str(item, "title");
            let url = json_str(item, "url");
            if title.is_empty() || url.is_empty() {
                continue;
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let description = json_str(item, "description");
            let mut builder = ResultBuilder::new(url, title).content(description);
            let age = json_str(item, "age");
            if !age.is_empty() {
                builder = builder.metadata("age", age);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_web_results() {
        let body = r#"{"web":{"results":[
          {"title":"Rust","url":"https://www.rust-lang.org/","description":"A language","age":"1d"}
        ]}}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = BraveApi::from_config(&EngineConfig::new(
            "braveapi",
            "braveapi",
            "braveapi",
            &["general"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn forwards_safesearch_and_time_range() {
        let engine = BraveApi::from_config(&EngineConfig::new(
            "braveapi",
            "braveapi",
            "braveapi",
            &["general"],
        ))
        .unwrap();
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.safesearch = 2;
        query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(request.url.contains("safesearch=strict"));
        assert!(request.url.contains("time_range=past_week"));
    }
}
