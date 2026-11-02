//! Mixcloud engine, ported from `searx/engines/mixcloud.py`.
// engine-id: mixcloud

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music"],
    shortcut: "mc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.mixcloud.com";

/// Mixcloud.
pub struct Mixcloud {
    meta: EngineMeta,
}

impl Mixcloud {
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

impl Engine for Mixcloud {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = (pageno.max(1) - 1) * 10;
        let query_part = query_string(&[("q", query.query_text())]);
        Ok(EngineRequest::get(format!(
            "{}/search/?{query_part}&type=cloudcast&limit=10&offset={offset}",
            self.base_url()
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "name");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(url.clone(), title)
                    .content(json_str(item, "user.name"))
                    .thumbnail(json_str(item, "pictures.medium"))
                    .published_date(json_str(item, "created_time"))
                    .metadata(
                        "iframe_src",
                        format!(
                            "https://www.mixcloud.com/widget/iframe/?feed={}",
                            encode(&url)
                        ),
                    )
                    .build(),
            );
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

    const FIXTURE: &str = r#"{
      "data": [
        {
          "url": "https://www.mixcloud.com/dj/a-mix/",
          "name": "A mix",
          "created_time": "2024-01-01T00:00:00Z",
          "pictures": {"medium": "https://thumbnailer.mixcloud.com/unsafe/100x100/picture.jpg"},
          "user": {"name": "A DJ"}
        }
      ]
    }"#;

    fn engine() -> Arc<Mixcloud> {
        Mixcloud::from_config(&EngineConfig::new("mixcloud", "mixcloud", "mc", &["music"])).unwrap()
    }

    #[test]
    fn parses_cloudcasts() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("mix", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A mix");
                assert_eq!(result.url, "https://www.mixcloud.com/dj/a-mix/");
                assert_eq!(result.content, "A DJ");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://thumbnailer.mixcloud.com/unsafe/100x100/picture.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("deep house", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert_eq!(
            request.url,
            "https://api.mixcloud.com/search/?q=deep%20house&type=cloudcast&limit=10&offset=20"
        );
    }
}
