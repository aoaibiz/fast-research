//! SepiaSearch engine, ported from `searx/engines/sepiasearch.py`.
//!
//! SepiaSearch uses the same API and result schema as Peertube, so the parsing
//! is reused from `crate::engines::peertube` (upstream imports it as well).
// engine-id: sepiasearch

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::engines::peertube::{start_date, video_response};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "sep",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://sepiasearch.org";

/// SepiaSearch, the Peertube search index.
pub struct Sepiasearch {
    meta: EngineMeta,
}

impl Sepiasearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Sepiasearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        if query.query_text().is_empty() {
            return Err(EngineError::Message("sepiasearch: empty query".into()));
        }
        let pairs: Vec<(&str, String)> = vec![
            ("search", query.query_text()),
            ("start", ((pageno.max(1) - 1) * 10).to_string()),
            ("count", "10".to_string()),
            ("sort", "-match".to_string()),
            (
                "nsfw",
                crate::engines::peertube::safesearch_value(query.safesearch).to_string(),
            ),
        ];
        let mut url = format!(
            "{}/api/v1/search/videos?{}",
            self.base_url(),
            query_string(&pairs)
        );
        let language = locale_language(&query.locale);
        if language != "all" {
            url.push_str(&format!("&languageOneOf[]={language}"));
            url.push_str(&format!("&boostLanguages[]={language}"));
        }
        if let Some(range) = query.time_range {
            if let Some(date) = start_date(range) {
                url.push_str(&format!("&startDate={date}"));
            }
        }
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        Ok(video_response(&json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::Template;

    const FIXTURE: &str = r#"{
      "total": 1,
      "data": [
        {
          "url": "https://framatube.org/w/abc123",
          "name": "A sepia video",
          "description": "A description",
          "duration": 60,
          "views": 999,
          "publishedAt": "2024-01-01T00:00:00.000Z",
          "embedUrl": "https://framatube.org/videos/embed/abc123",
          "thumbnailUrl": "https://framatube.org/thumb.jpg",
          "account": {"displayName": "An account"},
          "channel": {"displayName": "A channel", "name": "channel", "host": "framatube.org"},
          "tags": []
        }
      ]
    }"#;

    fn engine() -> Arc<Sepiasearch> {
        Sepiasearch::from_config(&EngineConfig::new(
            "sepiasearch",
            "sepiasearch",
            "sep",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_videos() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A sepia video");
                assert_eq!(result.url, "https://framatube.org/w/abc123");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("01:00")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(
            request
                .url
                .starts_with("https://sepiasearch.org/api/v1/search/videos?")
        );
        assert!(request.url.contains("start=20"));
        assert!(request.url.contains("sort=-match"));
        assert!(request.url.contains("nsfw=both"));
    }
}
