//! Podchaser engine, ported from searx/engines/podchaser.py.
//!
//! Upstream uses the public Podchaser podcast API without an API key; the
//! categories are only used to build the result metadata, exactly like there.
// engine-id: podchaser

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &[],
    shortcut: "poc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.podchaser.com";
const PAGE_SIZE: i64 = 25;
const ACCEPT: &str = "application/prs.podchaser.v2+json";

/// Podchaser podcast search.
pub struct Podchaser {
    meta: EngineMeta,
    page_size: i64,
}

impl Podchaser {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            page_size: config.opt_i64("page_size").unwrap_or(PAGE_SIZE),
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for Podchaser {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = i64::from(pageno.saturating_sub(1)) * self.page_size;
        let pairs: Vec<(&str, String)> = vec![
            ("filters[term]", query.query_text()),
            ("limit", self.page_size.to_string()),
            ("offset", offset.to_string()),
            ("sort_direction", "desc".to_string()),
            ("sort_order", "SORT_ORDER_RELEVANCE".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/podcasts?{}",
            self.base_url(),
            query_string(&pairs)
        ))
        .header("Accept", ACCEPT))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "entities").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "feed_url");
            let title = json_str(item, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "description"))
                .published_date(json_str(item, "created_at"));
            let image = json_str(item, "image_url");
            if !image.is_empty() {
                builder = builder.thumbnail(image);
            }
            let episodes = json_str(item, "number_of_episodes");
            if !episodes.is_empty() {
                builder = builder.metadata("episodes", format!("{episodes} episodes"));
            }
            let categories = categories_text(item);
            if !categories.is_empty() {
                builder = builder.metadata("categories", categories);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream joins the category texts with a comma and a space.
fn categories_text(item: &serde_json::Value) -> String {
    json_path(item, "categories")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|category| {
                    category
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"entities":[
      {"title":"Rustacean Radio","feed_url":"https://rustacean.example/feed.xml",
       "description":"A podcast about Rust.","image_url":"https://rustacean.example/cover.jpg",
       "created_at":"2021-03-04T05:06:07","number_of_episodes":12,
       "categories":[{"text":"Technology"},{"text":"Software"}]},
      {"title":"No feed","feed_url":"","description":"broken","number_of_episodes":1,
       "categories":[]}
    ]}"#;

    fn engine() -> Arc<Podchaser> {
        Podchaser::from_config(&EngineConfig::new("podchaser", "podchaser", "poc", &[])).unwrap()
    }

    #[test]
    fn parses_podcasts_with_episode_metadata() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://rustacean.example/feed.xml");
                assert_eq!(result.title, "Rustacean Radio");
                assert_eq!(result.content, "A podcast about Rust.");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://rustacean.example/cover.jpg")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2021-03-04T05:06:07")
                );
                assert_eq!(
                    result.metadata.get("episodes").map(String::as_str),
                    Some("12 episodes")
                );
                assert_eq!(
                    result.metadata.get("categories").map(String::as_str),
                    Some("Technology, Software")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_api_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.podchaser.com/podcasts?")
        );
        assert!(request.url.contains("filters%5Bterm%5D=rust"));
        assert!(request.url.contains("limit=25"));
        assert!(request.url.contains("offset=25"));
        let accept = request
            .headers
            .get("accept")
            .and_then(|value| value.to_str().ok());
        assert_eq!(accept, Some(ACCEPT));
    }
}
