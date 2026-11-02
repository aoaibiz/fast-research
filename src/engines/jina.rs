//! Jina engine, ported from searx/engines/jina.py.
//!
//! Requires an API key; the mode is selected with the jina_engine setting.
// engine-id: jina

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "ji",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://s.jina.ai";

/// Jina search, a JSON API in front of Jina's own index.
pub struct Jina {
    meta: EngineMeta,
}

impl Jina {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        if meta.api_key.as_deref().unwrap_or_default().is_empty() {
            return Err("jina requires an api key".to_string());
        }
        Ok(Arc::new(Self { meta }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }

    fn mode(&self) -> String {
        self.meta
            .opt_str("jina_engine")
            .unwrap_or("reader")
            .to_string()
    }
}

impl Engine for Jina {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let api_key = self.meta.api_key.as_deref().unwrap_or_default();
        if api_key.is_empty() {
            return Err(EngineError::Message("jina requires an api key".to_string()));
        }
        let pairs = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("engine", self.mode()),
            ("respondWith", "no-content".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{}/?{}", self.base_url(), query_string(&pairs)))
                .header("Accept", "application/json")
                .header("Authorization", &format!("Bearer {api_key}")),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "data").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let url = json_str(entry, "url");
            let title = json_str(entry, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(json_str(entry, "description"))
                    .published_date(date(&json_str(entry, "date")).unwrap_or_default())
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream parses the date with dateutil; ISO 8601 and RFC 2822 are accepted.
fn date(value: &str) -> Option<String> {
    let value = value.trim();
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(date.to_rfc3339());
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(value) {
        return Some(date.to_rfc3339());
    }
    if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(date.format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%dT00:00:00").to_string())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"code":200,"data":[
      {"url":"https://example.com/a","title":"Example A","description":"A snippet","date":"2024-02-03"},
      {"url":"ftp://example.com/b","title":"Bad","description":"x"},
      {"url":"https://example.com/c","title":"","description":"x"},
      {"url":"https://example.com/d","title":"No date","description":"x","date":"not a date"}
    ]}"##;

    fn engine() -> Arc<Jina> {
        let mut config = EngineConfig::new("jina", "jina", "ji", &["general"]);
        config.api_key = Some("jina_test".to_string());
        Jina::from_config(&config).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn response(body: &str) -> EngineResponse {
        EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec())
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("rust"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://s.jina.ai/?q=rust&page=2&engine=reader&respondWith=no-content"
        );
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        assert_eq!(authorization, Some("Bearer jina_test"));
        let accept = request
            .headers
            .get("accept")
            .and_then(|value| value.to_str().ok());
        assert_eq!(accept, Some("application/json"));
    }

    #[test]
    fn parses_the_results() {
        let items = engine().parse(&response(FIXTURE), &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/a");
        assert_eq!(result.title, "Example A");
        assert_eq!(result.content, "A snippet");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-02-03T00:00:00")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(second.url, "https://example.com/d");
        assert_eq!(second.published_date, None);
    }

    #[test]
    fn the_mode_is_configurable() {
        let mut config = EngineConfig::new("jina", "jina", "ji", &["general"]);
        config.api_key = Some("jina_test".to_string());
        config
            .extra
            .insert("jina_engine".into(), toml::Value::String("bing".into()));
        let engine = Jina::from_config(&config).unwrap();
        let request = engine.build_request(&query("rust"), 1).unwrap();
        assert!(
            request.url.contains("engine=bing"),
            "missing the mode in {}",
            request.url
        );
    }

    #[test]
    fn an_api_key_is_required() {
        let config = EngineConfig::new("jina", "jina", "ji", &["general"]);
        assert!(Jina::from_config(&config).is_err());
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        assert!(
            engine()
                .parse(&response(r##"{"code":401}"##), &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
