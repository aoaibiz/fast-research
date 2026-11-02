//! Devicons engine, ported from searx/engines/devicons.py.
// engine-id: devicons

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
    shortcut: "di",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const CDN_BASE: &str = "https://cdn.jsdelivr.net/gh/devicons/devicon@latest";

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn string_list(value: &serde_json::Value, path: &str) -> Vec<String> {
    json_path(value, path)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Upstream matches every query word against the name, altnames and tags.
fn is_match(icon: &serde_json::Value, parts: &[String]) -> bool {
    let name = json_str(icon, "name");
    if parts.iter().any(|part| name.contains(part.as_str())) {
        return true;
    }
    let mut words = string_list(icon, "altnames");
    words.extend(string_list(icon, "tags"));
    parts
        .iter()
        .any(|part| words.iter().any(|word| word.contains(part.as_str())))
}

/// Devicons, the icon set served from jsDelivr.
pub struct Devicons {
    meta: EngineMeta,
}

impl Devicons {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Devicons {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(CDN_BASE)
            .trim_end_matches('/');
        Ok(EngineRequest::get(format!("{base}/devicon.json")).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(icons) = json.as_array() else {
            return Ok(Vec::new());
        };
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(CDN_BASE)
            .trim_end_matches('/');
        let parts: Vec<String> = query
            .query_text()
            .to_lowercase()
            .split(' ')
            .map(str::to_string)
            .collect();
        let mut results = Vec::new();
        for icon in icons {
            if !is_match(icon, &parts) {
                continue;
            }
            let name = json_str(icon, "name");
            if name.is_empty() {
                continue;
            }
            let color = json_str(icon, "color");
            for image_type in string_list(icon, "versions.svg") {
                if image_type.is_empty() {
                    continue;
                }
                let img_src = format!("{base}/icons/{name}/{name}-{image_type}.svg");
                if !is_http(&img_src) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(img_src.clone(), name.clone())
                        .content(format!("Base color: {color}"))
                        .image(img_src)
                        .template(Template::Images)
                        .metadata("img_format", "SVG")
                        .build(),
                );
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"[
      {"name":"rust","altnames":["rust-lang"],"tags":["language"],
       "color":"\u0023dea584","versions":{"svg":["original","plain"],"font":["plain"]}},
      {"name":"python","altnames":["py"],"tags":["language"],
       "color":"\u0023377ab3","versions":{"svg":["original"]}},
      {"name":"broken","altnames":[],"tags":[],"color":"\u0023000000","versions":{}}
    ]"##;

    fn engine() -> Arc<Devicons> {
        Devicons::from_config(&EngineConfig::new("devicons", "devicons", "di", &["icons"])).unwrap()
    }

    fn parse(query_text: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, CDN_BASE, FIXTURE.as_bytes().to_vec());
        let query = Query::parse(query_text, &crate::engine::EngineRegistry::default(), &[]);
        engine().parse(&response, &query).unwrap()
    }

    #[test]
    fn emits_one_image_per_svg_version() {
        let items = parse("rust");
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected an icon result");
        };
        assert_eq!(
            result.url,
            "https://cdn.jsdelivr.net/gh/devicons/devicon@latest/icons/rust/rust-original.svg"
        );
        assert_eq!(result.title, "rust");
        assert_eq!(result.content, "Base color: #dea584");
        assert_eq!(result.img_src.as_deref(), Some(result.url.as_str()));
        assert_eq!(result.template, Template::Images);
        assert_eq!(
            result.metadata.get("img_format").map(String::as_str),
            Some("SVG")
        );
    }

    #[test]
    fn matches_altnames_and_skips_icons_without_svg() {
        let items = parse("lang");
        assert_eq!(items.len(), 3);
        assert!(parse("broken").is_empty());
    }

    #[test]
    fn builds_the_manifest_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://cdn.jsdelivr.net/gh/devicons/devicon@latest/devicon.json"
        );
    }
}
