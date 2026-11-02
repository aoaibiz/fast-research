//! PyPI engine, ported from `searx/engines/pypi.py`.

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
    shortcut: "pypi",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pypi.org";

/// PyPI package search (HTML endpoint).
pub struct PyPi {
    meta: EngineMeta,
}

impl PyPi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for PyPi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("page", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/search/?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "a.package-snippet")? {
            let Some(href) = attr(element, "href") else {
                continue;
            };
            let name = text_of(element, "span.package-snippet__name")
                .or_else(|| text_of(element, "h3 span"))
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let url = absolute_url(&href, BASE_URL)
                .unwrap_or_else(|| format!("{BASE_URL}/project/{name}/"));
            let content = text_of(element, "p.package-snippet__description").unwrap_or_default();
            results.push(
                ResultBuilder::new(url, name.clone())
                    .content(content)
                    .template(Template::Packages)
                    .metadata("package_name", name)
                    .build(),
            );
        }
        Ok(results)
    }
}
