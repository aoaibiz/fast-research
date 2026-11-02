//! Grokipedia engine, ported from searx/engines/grokipedia.py.
// engine-id: grokipedia

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "gp",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://grokipedia.com/api/full-text-search";
const PAGE_URL: &str = "https://grokipedia.com/page/";

/// Grokipedia, an encyclopedia with full text search.
pub struct Grokipedia {
    meta: EngineMeta,
}

impl Grokipedia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// Upstream's results_per_page, 10 unless configured otherwise.
    fn results_per_page(&self) -> u32 {
        match self.meta.opt_i64("results_per_page") {
            Some(value) if value > 0 => value as u32,
            _ => 10,
        }
    }
}

impl Engine for Grokipedia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let limit = self.results_per_page();
        let offset = pageno.saturating_sub(1).saturating_mul(limit);
        let pairs = vec![
            ("query", query.query_text()),
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
        ];
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        Ok(EngineRequest::get(format!(
            "{}?{}",
            base.trim_end_matches('/'),
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
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let slug = json_str(item, "slug");
            let title = json_str(item, "title");
            if slug.is_empty() || title.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(format!("{PAGE_URL}{slug}"), title)
                    .content(strip_tags(&json_str(item, "snippet")))
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Grokipedia> {
        Grokipedia::from_config(&EngineConfig::new(
            "grokipedia",
            "grokipedia",
            "gp",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_result_list() {
        let body = r##"{"results":[{"slug":"rust","title":"Rust","snippet":"A <b>language</b>."},{"slug":"","title":"no slug","snippet":""}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://grokipedia.com/page/rust");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn missing_results_yield_no_items() {
        let response = EngineResponse::new(200, BASE_URL, "{}".as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with(BASE_URL));
        assert!(request.url.contains("query=rust"));
        assert!(request.url.contains("limit=10"));
        assert!(request.url.contains("offset=20"));
    }
}
