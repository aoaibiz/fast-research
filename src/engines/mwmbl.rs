//! Mwmbl engine, ported from `searx/engines/mwmbl.py`.
//!
//! Mwmbl is a small, non-profit search engine; upstream does not support
//! regions, languages, safe search or time ranges, and neither do we.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "mwm",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.mwmbl.org/api/v1";

/// Mwmbl.
pub struct Mwmbl {
    meta: EngineMeta,
}

impl Mwmbl {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Mwmbl {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| API_URL.to_string());
        Ok(EngineRequest::get(format!(
            "{}/search/?s={}",
            base.trim_end_matches('/'),
            encode(&query.query_text())
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = json
            .as_array()
            .ok_or_else(|| EngineError::Parse("expected a JSON array".into()))?;
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            if url.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(url, join_fragments(item, "title"))
                    .content(join_fragments(item, "extract"))
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Join the `[{"value": "..."}]` fragments Mwmbl returns for text fields.
pub fn join_fragments(item: &serde_json::Value, key: &str) -> String {
    match item.get(key) {
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .map(|part| json_str(part, "value"))
            .collect::<Vec<_>>()
            .join(""),
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}
