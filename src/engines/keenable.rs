//! Keenable engine, ported from searx/engines/keenable.py.
//!
//! The API is a JSON POST. Without an API key the public endpoint is used;
//! with a key the authenticated endpoint and the X-API-KEY header are used.
// engine-id: keenable

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "keen",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.keenable.ai";
const DEFAULT_MODE: &str = "pro";

/// Keenable, a web search with a keyless mode.
pub struct Keenable {
    meta: EngineMeta,
}

impl Keenable {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// Upstream's keenable_mode setting.
    fn mode(&self) -> String {
        self.meta
            .opt_str("keenable_mode")
            .unwrap_or(DEFAULT_MODE)
            .to_string()
    }
}

impl Engine for Keenable {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let api_key = self.meta.api_key.as_deref().unwrap_or_default();
        let url = if api_key.is_empty() {
            format!("{base}/v1/search/public")
        } else {
            format!("{base}/v1/search")
        };
        let body = serde_json::json!({
            "query": query.query_text(),
            "mode": self.mode(),
        });
        let mut request = EngineRequest::post(url)
            .header("Accept", "application/json")
            .header("X-Keenable-Title", &crate::network::default_user_agent())
            .json_body(body);
        if !api_key.is_empty() {
            request.set_header("X-API-KEY", api_key);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "title");
            if url.is_empty() || title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut content = json_str(item, "description");
            if content.is_empty() {
                content = json_str(item, "snippet");
            }
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(date) = published_date(&json_str(item, "published_at")) {
                builder = builder.published_date(date);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Accept the ISO 8601 forms the API sends; upstream silently drops invalid
/// dates, which is what an empty result here means.
fn published_date(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let date = value.trim_end_matches('Z');
    let formats = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ];
    let valid = chrono::DateTime::parse_from_rfc3339(value).is_ok()
        || formats
            .iter()
            .any(|format| chrono::NaiveDateTime::parse_from_str(date, format).is_ok())
        || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok();
    valid.then(|| date.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        EngineConfig::new("keenable", "keenable", "keen", &["general"])
    }

    fn engine(config: &EngineConfig) -> Arc<Keenable> {
        Keenable::from_config(config).expect("keenable engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn the_public_endpoint_is_used_without_a_key() {
        let request = engine(&config()).build_request(&query("rust"), 1).unwrap();
        assert_eq!(request.url, "https://api.keenable.ai/v1/search/public");
        assert!(request.headers.get("x-api-key").is_none());
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["query"].as_str(), Some("rust"));
        assert_eq!(body["mode"].as_str(), Some("pro"));
    }

    #[test]
    fn an_api_key_switches_to_the_authenticated_endpoint() {
        let mut config = config();
        config.api_key = Some("secret".into());
        config
            .extra
            .insert("keenable_mode".into(), toml::Value::String("fast".into()));
        let request = engine(&config).build_request(&query("rust"), 1).unwrap();
        assert_eq!(request.url, "https://api.keenable.ai/v1/search");
        assert_eq!(
            request
                .headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("secret")
        );
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["mode"].as_str(), Some("fast"));
    }

    #[test]
    fn parses_the_result_list() {
        let body = r##"{"results":[{"url":"https://example.com/a","title":"A","description":"","snippet":"Snippet","published_at":"2024-01-02T03:04:05Z"},{"url":"ftp://example.com/b","title":"B","description":"x","snippet":"y"}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine(&config()).parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/a");
                assert_eq!(result.title, "A");
                assert_eq!(result.content, "Snippet");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-01-02T03:04:05")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_response_without_results_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            engine(&config())
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
