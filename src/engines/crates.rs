//! crates.io engine, ported from `searx/engines/crates.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "packages", "cargo"],
    shortcut: "crates",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://crates.io/api/v1/crates";

/// crates.io package search.
pub struct Crates {
    meta: EngineMeta,
}

impl Crates {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Crates {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.to_string()),
            ("per_page", "10".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "crates").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let name = json_str(item, "name");
            if name.is_empty() {
                continue;
            }
            let version = json_str(item, "max_version");
            let downloads = json_str(item, "downloads");
            let description = json_str(item, "description");
            let mut content = Vec::new();
            if !description.is_empty() {
                content.push(description);
            }
            if !version.is_empty() {
                content.push(format!("version {version}"));
            }
            if !downloads.is_empty() {
                content.push(format!("{downloads} downloads"));
            }
            results.push(
                ResultBuilder::new(format!("https://crates.io/crates/{name}"), name.clone())
                    .content(content.join(" | "))
                    .template(Template::Packages)
                    .published_date(json_str(item, "updated_at"))
                    .metadata("package_name", name)
                    .metadata("version", version)
                    .build(),
            );
        }
        Ok(results)
    }
}
