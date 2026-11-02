//! Selfh.st icons engine, ported from searx/engines/selfhst.py.
//!
//! The whole icon manifest is fetched once per query and filtered locally,
//! exactly like upstream.
// engine-id: selfhst

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
    shortcut: "si",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const CDN_BASE: &str = "https://cdn.jsdelivr.net/gh/selfhst/icons";

/// The formats upstream probes, in order. The second value is the lower case
/// suffix used in the CDN path and in the img_format metadata.
const FORMATS: &[(&str, &str)] = &[("SVG", "svg"), ("PNG", "png"), ("WebP", "webp")];

/// The selfh.st icon collection, served from jsDelivr.
pub struct Selfhst {
    meta: EngineMeta,
}

impl Selfhst {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn cdn_base(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(CDN_BASE)
            .trim_end_matches('/')
    }
}

impl Engine for Selfhst {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(
            EngineRequest::get(format!("{}/index.json", self.cdn_base()))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let base = self.cdn_base();
        let parts: Vec<String> = query
            .query_text()
            .to_lowercase()
            .split(' ')
            .map(str::to_string)
            .collect();
        let mut results = Vec::new();
        for item in items {
            let reference = json_str(item, "Reference");
            let keyword = reference.to_lowercase();
            if keyword.is_empty() {
                continue;
            }
            // An empty query part matches everything, like Python's
            // substring test does.
            if !parts
                .iter()
                .any(|part| part.is_empty() || keyword.contains(part.as_str()))
            {
                continue;
            }
            let title = json_str(item, "Name");
            if title.is_empty() {
                continue;
            }
            let mut img_format = String::new();
            for (label, suffix) in FORMATS.iter().copied() {
                if json_str(item, label) == "Yes" {
                    img_format = suffix.to_string();
                    break;
                }
            }
            // Upstream builds a URL with a "None" format when no format is
            // available; such an icon is skipped here.
            if img_format.is_empty() {
                continue;
            }
            let img_src = format!("{base}/{img_format}/{reference}.{img_format}");
            if !is_http(&img_src) {
                continue;
            }
            let mut builder = ResultBuilder::new(img_src.clone(), title)
                .template(Template::Images)
                .image(img_src)
                .metadata("img_format", img_format);
            let created = json_str(item, "CreatedAt");
            if !created.is_empty() {
                builder = builder.published_date(created);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"[
      {"Reference":"rust","Name":"Rust","SVG":"Yes","PNG":"Yes","WebP":"No","CreatedAt":"2024-01-02T03:04:05Z"},
      {"Reference":"python","Name":"Python","SVG":"No","PNG":"Yes","WebP":"No","CreatedAt":"2024-02-03T00:00:00Z"},
      {"Reference":"broken","Name":"Broken","SVG":"No","PNG":"No","WebP":"No","CreatedAt":"2024-02-03T00:00:00Z"},
      {"Reference":"nameless","Name":"","SVG":"Yes","PNG":"No","WebP":"No","CreatedAt":"2024-02-03T00:00:00Z"}
    ]"##;

    fn engine() -> Arc<Selfhst> {
        Selfhst::from_config(&EngineConfig::new("selfhst", "selfhst", "si", &["icons"])).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_matching_icons() {
        let response = EngineResponse::new(200, CDN_BASE, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://cdn.jsdelivr.net/gh/selfhst/icons/svg/rust.svg"
                );
                assert_eq!(result.title, "Rust");
                assert_eq!(result.img_src.as_deref(), Some(result.url.as_str()));
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.metadata.get("img_format").map(String::as_str),
                    Some("svg")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-01-02T03:04:05Z")
                );
            }
            _ => panic!("expected an icon result"),
        }
    }

    #[test]
    fn falls_back_to_the_next_format() {
        let response = EngineResponse::new(200, CDN_BASE, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("python")).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://cdn.jsdelivr.net/gh/selfhst/icons/png/python.png"
                );
                assert_eq!(
                    result.metadata.get("img_format").map(String::as_str),
                    Some("png")
                );
            }
            _ => panic!("expected an icon result"),
        }
    }

    #[test]
    fn skips_unusable_icons() {
        let response = EngineResponse::new(200, CDN_BASE, FIXTURE.as_bytes().to_vec());
        assert!(
            engine()
                .parse(&response, &query("broken"))
                .unwrap()
                .is_empty()
        );
        assert!(
            engine()
                .parse(&response, &query("nameless"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn builds_the_manifest_request() {
        let request = engine().build_request(&query("rust"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://cdn.jsdelivr.net/gh/selfhst/icons/index.json"
        );
    }
}
