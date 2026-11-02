//! Wikidata engine, ported from `searx/engines/wikidata.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "wd",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://www.wikidata.org/w/api.php";

/// Wikidata entity search.
pub struct Wikidata {
    meta: EngineMeta,
}

impl Wikidata {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Wikidata {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let pairs: Vec<(&str, String)> = vec![
            ("action", "wbsearchentities".to_string()),
            ("search", query.query_text()),
            ("language", language.clone()),
            ("uselang", language),
            ("limit", "10".to_string()),
            ("format", "json".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{API_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json.get("search").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let id = json_str(item, "id");
            if id.is_empty() {
                continue;
            }
            let title = {
                let label = json_str(item, "label");
                if label.is_empty() { id.clone() } else { label }
            };
            results.push(
                ResultBuilder::new(format!("https://www.wikidata.org/wiki/{id}"), title)
                    .content(json_str(item, "description"))
                    .build(),
            );
        }
        Ok(results)
    }
}
