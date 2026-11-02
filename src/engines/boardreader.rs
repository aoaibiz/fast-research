//! Boardreader engine, ported from searx/engines/boardreader.py.

// engine-id: boardreader

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "social media"],
    shortcut: "boa",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://boardreader.com";

/// Boardreader, the forum search engine.
pub struct Boardreader {
    meta: EngineMeta,
}

impl Boardreader {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Boardreader {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        // Upstream scrapes a session id from the home page first; the port
        // takes it from the settings (session_id or api_key).
        let session_id = self
            .meta
            .opt_str("session_id")
            .or(self.meta.api_key.as_deref())
            .unwrap_or_default();
        let language = match self.meta.opt_str("language") {
            Some(language) => language.to_string(),
            None => {
                let language = locale_language(&query.locale);
                if language == "all" {
                    "All".to_string()
                } else {
                    language
                }
            }
        };
        let mut pairs = vec![
            ("query", query.query_text()),
            ("page", pageno.to_string()),
            ("language", language),
            ("session_id", session_id.to_string()),
        ];
        if let Some(days) = query.time_range.and_then(period_days) {
            pairs.push(("period", days.to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{base}/return.php?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "SearchResults").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = remove_keyword_marker(&json_str(item, "Subject"));
            let url = json_str(item, "Url");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let content = remove_keyword_marker(&json_str(item, "Text"));
            let mut builder = ResultBuilder::new(url, title).content(content);
            let author = json_str(item, "Author");
            if !author.is_empty() {
                builder = builder.author(author);
            }
            let published = json_str(item, "Published");
            if !published.is_empty() {
                builder = builder.published_date(published);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream converts [Keyword]ABCDE[/Keyword] to ABCDE.
fn remove_keyword_marker(value: &str) -> String {
    let cleaned = value.replace("[Keyword]", "").replace("[/Keyword]", "");
    strip_tags(&cleaned)
}

fn period_days(range: TimeRange) -> Option<i64> {
    match range {
        TimeRange::Day => Some(1),
        TimeRange::Week => Some(7),
        TimeRange::Month => Some(30),
        TimeRange::Year => Some(365),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"SearchResults": [
      {"Subject": "[Keyword]rust[/Keyword] async", "Text": "<b>Tokio</b> runtime",
       "Url": "https://example.com/threads/1", "Published": "2024-05-01T10:00:00", "Author": "alice"},
      {"Subject": "Second thread", "Text": "Another answer",
       "Url": "https://example.com/threads/2", "Published": "2024-05-02T10:00:00", "Author": "bob"},
      {"Subject": "Broken link", "Text": "no url"}
    ]}"##;

    fn engine() -> Arc<Boardreader> {
        let mut config = EngineConfig::new(
            "boardreader",
            "boardreader",
            "boa",
            &["general", "social media"],
        );
        config.extra.insert(
            "session_id".to_string(),
            toml::Value::String("abc123".to_string()),
        );
        Boardreader::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_search_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "rust async");
                assert_eq!(result.content, "Tokio runtime");
                assert_eq!(result.url, "https://example.com/threads/1");
                assert_eq!(result.author.as_deref(), Some("alice"));
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T10:00:00")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_query_with_session_and_period() {
        let mut query = query();
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://boardreader.com/return.php?query=rust&page=2&language=All&session_id=abc123&period=7"
        );
    }

    #[test]
    fn forwards_the_locale_language() {
        let mut query = query();
        query.locale = "it-IT".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.contains("&language=it&"));
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
