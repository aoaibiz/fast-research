//! Lucide engine, ported from searx/engines/lucide.py.
//!
//! The whole tag catalogue is downloaded once per request and filtered
//! locally, like upstream does.
// engine-id: lucide

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
    shortcut: "luc",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const CDN_BASE_URL: &str = "https://cdn.jsdelivr.net/npm/lucide-static";

/// Lucide, a copyleft icon collection.
pub struct Lucide {
    meta: EngineMeta,
}

impl Lucide {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn cdn_base(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(CDN_BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Lucide {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // The query is applied locally, the CDN only serves the catalogue.
        Ok(EngineRequest::get(format!("{}/tags.json", self.cdn_base()))
            .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(catalogue) = json.as_object() else {
            return Ok(Vec::new());
        };
        let cdn = self.cdn_base();
        let parts: Vec<String> = query
            .query_text()
            .to_lowercase()
            .split(' ')
            .map(str::to_string)
            .collect();
        let mut results = Vec::new();
        for (name, value) in catalogue {
            if name.is_empty() {
                continue;
            }
            let tags = string_list(value);
            if !matches(name, &tags, &parts) {
                continue;
            }
            let img_src = format!("{cdn}/icons/{name}.svg");
            if !is_http_url(&img_src) {
                continue;
            }
            results.push(
                ResultBuilder::new(img_src.clone(), name.clone())
                    .content(tags.join(", "))
                    .image(img_src)
                    .template(Template::Images)
                    .metadata("img_format", "SVG")
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream matches a query word against the icon name and against every tag.
fn matches(name: &str, tags: &[String], parts: &[String]) -> bool {
    parts.iter().any(|part| {
        name.contains(part.as_str()) || tags.iter().any(|tag| tag.contains(part.as_str()))
    })
}

fn string_list(value: &serde_json::Value) -> Vec<String> {
    match value.as_array() {
        Some(items) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        r##"{"home": ["house", "building"], "user": ["person"], "alert-circle": ["warning"]}"##;

    fn engine() -> Arc<Lucide> {
        Lucide::from_config(&EngineConfig::new("lucide", "lucide", "luc", &["icons"])).unwrap()
    }

    fn query(query_text: &str) -> Query {
        Query::parse(query_text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn parse(query_text: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, CDN_BASE_URL, FIXTURE.as_bytes().to_vec());
        engine().parse(&response, &query(query_text)).unwrap()
    }

    #[test]
    fn matches_the_icon_name() {
        let items = parse("home");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                let url = "https://cdn.jsdelivr.net/npm/lucide-static/icons/home.svg";
                assert_eq!(result.url, url);
                assert_eq!(result.title, "home");
                assert_eq!(result.content, "house, building");
                assert_eq!(result.img_src.as_deref(), Some(url));
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.metadata.get("img_format").map(String::as_str),
                    Some("SVG")
                );
            }
            _ => panic!("expected an icon result"),
        }
    }

    #[test]
    fn matches_a_tag() {
        let items = parse("warn");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => assert_eq!(result.title, "alert-circle"),
            _ => panic!("expected an icon result"),
        }
        assert!(parse("nosuchicon").is_empty());
    }

    #[test]
    fn builds_the_catalogue_request() {
        let request = engine().build_request(&query("home"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://cdn.jsdelivr.net/npm/lucide-static/tags.json"
        );
    }

    #[test]
    fn a_non_object_response_is_empty() {
        let response = EngineResponse::new(200, CDN_BASE_URL, b"[]".to_vec());
        assert!(
            engine()
                .parse(&response, &query("home"))
                .unwrap()
                .is_empty()
        );
    }
}
