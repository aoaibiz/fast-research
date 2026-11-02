//! Frinkiac engine, ported from searx/engines/frinkiac.py.

// engine-id: frinkiac

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "frk",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://frinkiac.com";

/// Frinkiac, the Simpsons screenshot search.
pub struct Frinkiac {
    meta: EngineMeta,
}

impl Frinkiac {
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
}

impl Engine for Frinkiac {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("q", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}/api/search?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json.as_array() {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for item in items {
            let episode = json_str(item, "Episode");
            let timestamp = json_str(item, "Timestamp");
            if episode.is_empty() || timestamp.is_empty() {
                continue;
            }
            let pairs = vec![
                ("p", "caption".to_string()),
                ("e", episode.clone()),
                ("t", timestamp.clone()),
            ];
            let url = format!("{base}/?{}", query_string(&pairs));
            if !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, episode.clone())
                    .thumbnail(format!("{base}/img/{episode}/{timestamp}/medium.jpg"))
                    .image(format!("{base}/img/{episode}/{timestamp}.jpg"))
                    .template(Template::Images)
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
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"[
      {"Episode":"S05E10","Timestamp":123456},
      {"Episode":"S01E01","Timestamp":7},
      {"Episode":"","Timestamp":1}
    ]"##;

    fn engine() -> Arc<Frinkiac> {
        Frinkiac::from_config(&EngineConfig::new(
            "frinkiac",
            "frinkiac",
            "frk",
            &["images"],
        ))
        .unwrap()
    }

    fn search() -> Query {
        Query::parse("hello there", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&search(), 1).unwrap();
        assert_eq!(
            request.url,
            "https://frinkiac.com/api/search?q=hello%20there"
        );
    }

    #[test]
    fn parses_screenshots() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://frinkiac.com/?p=caption&e=S05E10&t=123456"
                );
                assert_eq!(result.title, "S05E10");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://frinkiac.com/img/S05E10/123456/medium.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://frinkiac.com/img/S05E10/123456.jpg")
                );
                assert_eq!(result.template, Template::Images);
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
