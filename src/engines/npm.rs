//! npm engine, ported from `searx/engines/npm.py`.
//!
//! Upstream queries the npms.io API; we use the npm registry search endpoint,
//! which is the maintained one (documented in docs/PORTING.md).

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "packages"],
    shortcut: "npm",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://api.npms.io/v2/search";

/// npm package search.
pub struct Npm {
    meta: EngineMeta,
}

impl Npm {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Npm {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            // Upstream (npms.io) pages 25 results at a time.
            ("size", "25".to_string()),
            ("from", ((pageno - 1) * 25).to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_API}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "results").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let name = json_str(item, "package.name");
            if name.is_empty() {
                continue;
            }
            let mut url = json_str(item, "package.links.npm");
            if url.is_empty() {
                url = format!("https://www.npmjs.com/package/{name}");
            }
            let version = json_str(item, "package.version");
            let description = json_str(item, "package.description");
            let mut content = Vec::new();
            if !description.is_empty() {
                content.push(description);
            }
            if !version.is_empty() {
                content.push(format!("version {version}"));
            }
            results.push(
                ResultBuilder::new(url, name.clone())
                    .content(content.join(" | "))
                    .template(Template::Packages)
                    .published_date(json_str(item, "package.date"))
                    .author(json_str(item, "package.publisher.username"))
                    .metadata("package_name", name)
                    .metadata("version", version)
                    .build(),
            );
        }
        Ok(results)
    }
}
