//! Docker Hub engine, ported from `searx/engines/docker_hub.py`.

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
    shortcut: "dh",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://hub.docker.com";

/// Docker Hub image search.
pub struct DockerHub {
    meta: EngineMeta,
}

impl DockerHub {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for DockerHub {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("from", ((pageno - 1) * 10).to_string()),
            ("size", "10".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/api/search/v3/catalog/search?{}",
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
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
            let slug = json_str(item, "slug");
            let name = {
                let name = json_str(item, "name");
                if name.is_empty() { slug.clone() } else { name }
            };
            if slug.is_empty() || name.is_empty() {
                continue;
            }
            let official = matches!(json_str(item, "source").as_str(), "store" | "official");
            let url = format!("{BASE_URL}/{}{slug}", if official { "_/" } else { "r/" });
            let stars = json_str(item, "star_count");
            let pulls = json_path(item, "rate_plans.0.repositories.0.pull_count")
                .map(|value| match value {
                    serde_json::Value::Number(number) => number.to_string(),
                    other => other.as_str().unwrap_or_default().to_string(),
                })
                .unwrap_or_default();
            let mut content = Vec::new();
            let description = json_str(item, "short_description");
            if !description.is_empty() {
                content.push(description);
            }
            if !pulls.is_empty() {
                content.push(format!("{pulls} pulls"));
            }
            content.push(format!("{stars} stars"));
            let mut builder = ResultBuilder::new(url, name.clone())
                .content(content.join(" | "))
                .template(Template::Packages)
                .published_date(json_str(item, "updated_at"))
                .author(json_str(item, "publisher.name"))
                .metadata("package_name", name)
                .metadata("stars", stars);
            let thumb = json_str(item, "logo_url.large");
            let thumb = if thumb.is_empty() {
                json_str(item, "logo_url.small")
            } else {
                thumb
            };
            if !thumb.is_empty() {
                builder = builder.thumbnail(thumb);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}
