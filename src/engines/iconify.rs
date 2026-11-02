//! Iconify engine, ported from searx/engines/iconify.py.
//!
//! Iconify aggregates icons from different open source icon sets.
// engine-id: iconify

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "icons"],
    shortcut: "ico",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.iconify.design";
/// Upstream: page_size = 20.
const PAGE_SIZE: i64 = 20;

pub struct Iconify {
    meta: EngineMeta,
}

impl Iconify {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The base URL without its trailing slash.
    fn base(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Iconify {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let page_size = self.meta.opt_i64("page_size").unwrap_or(PAGE_SIZE).max(1);
        let start = i64::from(pageno.max(1) - 1) * page_size;
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("start", start.to_string()),
            ("limit", page_size.to_string()),
        ];
        let url = format!("{}/search?{}", self.base(), query_string(&pairs));
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(icons) = json.get("icons").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = self.base();
        let mut results = Vec::new();
        for icon in icons {
            let Some(icon) = icon.as_str() else {
                continue;
            };
            // The API names an icon "prefix:name".
            let Some((source, name)) = icon.split_once(':') else {
                continue;
            };
            if source.is_empty() || name.is_empty() {
                continue;
            }
            let url = format!("{base}/{source}/{name}.svg");
            results.push(
                ResultBuilder::new(url.clone(), name.to_string())
                    .image(url)
                    .template(Template::Images)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"icons": ["mdi:home", "tabler:user", "broken"], "total": 3}"##;

    fn engine() -> Arc<Iconify> {
        Iconify::from_config(&EngineConfig::new("iconify", "iconify", "ico", &["icons"])).unwrap()
    }

    #[test]
    fn parses_icon_names() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("home", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                let url = "https://api.iconify.design/mdi/home.svg";
                assert_eq!(result.url, url);
                assert_eq!(result.title, "home");
                assert_eq!(result.img_src.as_deref(), Some(url));
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an icon result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://api.iconify.design/tabler/user.svg");
                assert_eq!(result.title, "user");
            }
            _ => panic!("expected an icon result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert_eq!(
            request.url,
            "https://api.iconify.design/search?query=cat&start=40&limit=20"
        );
    }

    #[test]
    fn honours_the_page_size() {
        let mut config = EngineConfig::new("iconify", "iconify", "ico", &["icons"]);
        config
            .extra
            .insert("page_size".to_string(), toml::Value::Integer(10));
        let engine = Iconify::from_config(&config).unwrap();
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(request.url.contains("start=10"));
        assert!(request.url.contains("limit=10"));
    }
}
