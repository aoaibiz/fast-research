//! ScanR structures engine, ported from searx/engines/scanr_structures.py.

// engine-id: scanr_structures

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science"],
    shortcut: "scs",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://scanr.enseignementsup-recherche.gouv.fr";
const PAGE_SIZE: u32 = 20;

/// The ScanR open data API, limited to research structures.
pub struct ScanrStructures {
    meta: EngineMeta,
}

impl ScanrStructures {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }

    fn search_url(&self) -> String {
        self.meta
            .search_url
            .clone()
            .unwrap_or_else(|| format!("{}/api/structures/search", self.base_url()))
    }

    fn page_size(&self) -> i64 {
        self.meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(i64::from(PAGE_SIZE))
    }
}

impl Engine for ScanrStructures {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = serde_json::json!({
            "query": query.query_text(),
            "searchField": "ALL",
            "sortDirection": "ASC",
            "sortOrder": "RELEVANCY",
            "page": pageno,
            "pageSize": self.page_size(),
        });
        Ok(EngineRequest::post(self.search_url())
            .json_body(body)
            .header("Content-Type", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let total = json_path(&json, "total")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if total < 1 {
            return Ok(Vec::new());
        }
        let Some(entries) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for entry in entries {
            let id = json_str(entry, "id");
            let title = json_str(entry, "label");
            if id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{base}/structure/{}", encode(&id));
            let content = json_path(entry, "highlights.0.value")
                .and_then(|value| value.as_str())
                .map(strip_tags)
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(logo) = absolute_url(&json_str(entry, "logo"), &base) {
                if logo.starts_with("http://") || logo.starts_with("https://") {
                    builder = builder.thumbnail(logo);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"total": 4, "results": [
      {"id": "struct-1", "label": "Laboratory one", "logo": "/img/one.png",
       "highlights": [{"value": "A <b>research</b> lab"}]},
      {"id": "struct-2", "label": "Laboratory two"},
      {"id": "", "label": "Without an id"},
      {"id": "struct-3", "label": ""}
    ]}"##;

    fn engine() -> Arc<ScanrStructures> {
        ScanrStructures::from_config(&EngineConfig::new(
            "scanr structures",
            "scanr_structures",
            "scs",
            &["science"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("cancer", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_structure_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Laboratory one");
                assert_eq!(
                    result.url,
                    "https://scanr.enseignementsup-recherche.gouv.fr/structure/struct-1"
                );
                assert_eq!(result.content, "A research lab");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://scanr.enseignementsup-recherche.gouv.fr/img/one.png")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://scanr.enseignementsup-recherche.gouv.fr/structure/struct-2"
                );
                assert!(result.content.is_empty());
                assert!(result.thumbnail.is_none());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_json_request() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://scanr.enseignementsup-recherche.gouv.fr/api/structures/search"
        );
        let body = request.json.expect("a JSON body");
        assert_eq!(body["query"], serde_json::json!("cancer"));
        assert_eq!(body["page"], serde_json::json!(2));
        assert_eq!(body["pageSize"], serde_json::json!(20));
        assert_eq!(body["searchField"], serde_json::json!("ALL"));
        assert!(request.form.is_empty());
    }

    #[test]
    fn empty_hits_yield_no_results() {
        let response =
            EngineResponse::new(200, BASE_URL, br#"{"total": 0, "results": []}"#.to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
