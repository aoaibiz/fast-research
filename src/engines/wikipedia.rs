//! Wikipedia engine, ported from `searx/engines/wikipedia.py`.
//!
//! The MediaWiki search API is used (upstream additionally fetches the REST
//! summary for the infobox; that part is documented as not ported).

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "wp",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// Wikipedia.
pub struct Wikipedia {
    meta: EngineMeta,
}

impl Wikipedia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The wiki host for a language, e.g. `ja.wikipedia.org`.
    fn host(&self, query: &Query) -> String {
        if let Some(base) = &self.meta.base_url {
            return base.trim_end_matches('/').to_string();
        }
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        format!("https://{language}.wikipedia.org")
    }
}

impl Engine for Wikipedia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("action", "query".to_string()),
            ("list", "search".to_string()),
            ("srsearch", query.query_text()),
            ("srlimit", "10".to_string()),
            ("sroffset", ((pageno - 1) * 10).to_string()),
            ("format", "json".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/w/api.php?{}",
            self.host(query),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "query.search").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let host = self.host(query);
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            let page_id = json_str(item, "pageid");
            let url = if page_id.is_empty() {
                format!("{host}/wiki/{}", encode(&title.replace(' ', "_")))
            } else {
                format!("{host}/?curid={page_id}")
            };
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(item, "snippet")))
                    .build(),
            );
        }
        Ok(results)
    }
}
