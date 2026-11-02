//! Reuters engine, ported from searx/engines/reuters.py.

// engine-id: reuters

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "reuters",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.reuters.com";
const SEARCH_PATH: &str = "/pf/api/v3/content/fetch/articles-by-search-v2";
const RESULTS_PER_PAGE: u32 = 20;
const DEFAULT_SORT_ORDER: &str = "relevance";

/// Reuters, the international news agency.
pub struct Reuters {
    meta: EngineMeta,
}

impl Reuters {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }

    fn sort_order(&self) -> String {
        self.meta
            .opt_str("sort_order")
            .unwrap_or(DEFAULT_SORT_ORDER)
            .to_string()
    }
}

impl Engine for Reuters {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut args = serde_json::json!({
            "keyword": query.query_text(),
            "offset": pageno.saturating_sub(1) * RESULTS_PER_PAGE,
            "orderby": self.sort_order(),
            "size": RESULTS_PER_PAGE,
            "website": "reuters",
        });
        if let Some(days) = query.time_range.and_then(range_days) {
            let start = chrono::Local::now() - chrono::Duration::days(days);
            let start = start.naive_local().format("%Y-%m-%dT%H:%M:%S").to_string();
            args["start_date"] = serde_json::Value::String(start);
        }
        Ok(EngineRequest::get(format!(
            "{}{SEARCH_PATH}?query={}",
            self.base_url(),
            encode(&args.to_string())
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let articles = match json_path(&json, "result.articles").and_then(|value| value.as_array())
        {
            Some(articles) => articles,
            None => return Ok(Vec::new()),
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for article in articles {
            let title = json_str(article, "web");
            let canonical = json_str(article, "canonical_url");
            let url = if canonical.starts_with("http://") || canonical.starts_with("https://") {
                canonical
            } else if canonical.is_empty() {
                String::new()
            } else {
                format!("{base}/{}", canonical.trim_start_matches('/'))
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(article, "description"))
                .published_date(json_str(article, "display_time"))
                .metadata("kicker", json_str(article, "kicker.name"));
            let resizer = json_str(article, "thumbnail.resizer_url");
            if !resizer.is_empty() {
                let resizer = absolute_url(&resizer, &base).unwrap_or(resizer);
                builder = builder.thumbnail(format!("{resizer}&height=80"));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn range_days(range: TimeRange) -> Option<i64> {
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

    const FIXTURE: &str = r##"{"result": {"articles": [
      {"canonical_url": "/world/one-2024-10-24/", "web": "First story",
       "description": "The first story", "display_time": "2024-10-24T09:00:00Z",
       "kicker": {"name": "World"},
       "thumbnail": {"resizer_url": "https://cdn.example.test/x.jpg?auth=1"}},
      {"canonical_url": "https://www.reuters.com/tech/two/", "web": "Second story",
       "description": "The second story", "display_time": "2024-10-23T09:00:00Z"},
      {"canonical_url": "/broken/", "web": ""}
    ]}}"##;

    fn engine() -> Arc<Reuters> {
        Reuters::from_config(&EngineConfig::new("reuters", "reuters", "reu", &["news"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_article_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First story");
                assert_eq!(result.url, "https://www.reuters.com/world/one-2024-10-24/");
                assert_eq!(result.content, "The first story");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-10-24T09:00:00Z")
                );
                assert_eq!(
                    result.metadata.get("kicker").map(String::as_str),
                    Some("World")
                );
                assert!(
                    result
                        .thumbnail
                        .as_deref()
                        .unwrap_or_default()
                        .ends_with("&height=80")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert!(result.thumbnail.is_none()),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_query() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert!(request.url.starts_with(
            "https://www.reuters.com/pf/api/v3/content/fetch/articles-by-search-v2?query="
        ));
        assert!(request.url.contains("%22offset%22%3A20"));
        assert!(!request.url.contains("start_date"));
    }

    #[test]
    fn time_range_becomes_a_start_date() {
        let mut query = query();
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.contains("start_date"));
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
