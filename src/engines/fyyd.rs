//! Fyyd engine, ported from searx/engines/fyyd.py.

// engine-id: fyyd

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &[],
    shortcut: "fy",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 8.0,
};

const BASE_URL: &str = "https://api.fyyd.de";
const PAGE_SIZE: i64 = 10;

/// Fyyd, a podcast directory.
pub struct Fyyd {
    meta: EngineMeta,
}

impl Fyyd {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Fyyd {
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
        let size = self
            .meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(PAGE_SIZE);
        let pairs = vec![
            ("term", query.query_text()),
            ("count", size.to_string()),
            ("page", (pageno.max(1) - 1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{base}/0.2/search/podcast?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "data").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "htmlURL");
            let title = clean(&json_str(item, "title"));
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let rank = json_str(item, "rank");
            let episodes = json_str(item, "episode_count");
            let builder = ResultBuilder::new(url, title)
                .content(clean(&json_str(item, "description")))
                .thumbnail(json_str(item, "smallImageURL"))
                .published_date(json_str(item, "status_since"))
                .metadata("rank", rank)
                .metadata(
                    "episodes",
                    if episodes.is_empty() {
                        String::new()
                    } else {
                        format!("{episodes} episodes")
                    },
                );
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"{"data":[
      {"htmlURL":"https://fyyd.de/podcast/example","title":"  Example   Podcast ",
       "description":"A show about examples.","smallImageURL":"https://cdn.example.com/small.jpg",
       "status_since":"2024-01-02T03:04:05Z","rank":"3","episode_count":"12"},
      {"htmlURL":"https://fyyd.de/podcast/broken","title":"","rank":"1","episode_count":"0"}
    ]}"##;

    fn engine() -> Arc<Fyyd> {
        Fyyd::from_config(&EngineConfig::new("fyyd", "fyyd", "fy", &["general"])).unwrap()
    }

    fn search() -> Query {
        Query::parse("examples", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&search(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://api.fyyd.de/0.2/search/podcast?term=examples&count=10&page=1"
        );
    }

    #[test]
    fn parses_podcasts() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://fyyd.de/podcast/example");
                assert_eq!(result.title, "Example Podcast");
                assert_eq!(result.content, "A show about examples.");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.example.com/small.jpg")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-01-02T03:04:05Z")
                );
                assert_eq!(result.metadata.get("rank").map(String::as_str), Some("3"));
                assert_eq!(
                    result.metadata.get("episodes").map(String::as_str),
                    Some("12 episodes")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn missing_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &search()).unwrap().is_empty());
    }
}
