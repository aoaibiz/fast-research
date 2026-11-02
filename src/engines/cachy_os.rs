//! CachyOS engine, ported from `searx/engines/cachy_os.py`.

// engine-id: cachy_os

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["packages", "it"],
    shortcut: "cachy_os",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://packages.cachyos.org/api/search";
const RESULTS_PER_PAGE: u32 = 15;

/// CachyOS package search.
pub struct CachyOs {
    meta: EngineMeta,
}

impl CachyOs {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for CachyOs {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("search", query.query_text()),
            ("page_size", RESULTS_PER_PAGE.to_string()),
            ("current_page", pageno.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "packages").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let package_name = json_str(item, "pkg_name");
            if package_name.is_empty() {
                continue;
            }
            let arch = json_str(item, "pkg_arch");
            let repo = json_str(item, "repo_name");
            let url = format!("https://packages.cachyos.org/package/{repo}/{arch}/{package_name}");
            let mut builder = ResultBuilder::new(url, format!("{package_name} ({repo})"))
                .content(json_str(item, "pkg_desc"))
                .template(Template::Packages)
                .metadata("package_name", package_name.clone())
                .metadata("version", json_str(item, "pkg_version"));
            if !arch.is_empty() {
                builder = builder.metadata("tags", arch);
            }
            builder = builder.published_date(json_str(item, "pkg_builddate"));
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_json() {
        let body = r#"{"packages":[
          {"pkg_name":"firefox","pkg_arch":"x86_64","repo_name":"kde-unstable",
           "pkg_version":"130.0","pkg_desc":"Standalone web browser","pkg_builddate":"1720000000"}
        ]}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = CachyOs::from_config(&EngineConfig::new(
            "cachy os",
            "cachy_os",
            "cachy_os",
            &["packages"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("firefox", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://packages.cachyos.org/package/kde-unstable/x86_64/firefox"
                );
                assert_eq!(result.title, "firefox (kde-unstable)");
            }
            _ => panic!("expected a package result"),
        }
    }
}
