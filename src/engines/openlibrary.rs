//! Open Library engine, ported from `searx/engines/openlibrary.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "files"],
    shortcut: "ol",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://openlibrary.org/search.json";

/// Open Library book search.
pub struct OpenLibrary {
    meta: EngineMeta,
}

impl OpenLibrary {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for OpenLibrary {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.to_string()),
            ("limit", "10".to_string()),
            (
                "fields",
                "key,title,author_name,first_publish_year,edition_count,ia".to_string(),
            ),
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
        let items = match json_path(&json, "docs").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            let key = json_str(item, "key");
            if title.is_empty() || key.is_empty() {
                continue;
            }
            let author = item
                .get("author_name")
                .and_then(|value| value.as_array())
                .map(|authors| {
                    authors
                        .iter()
                        .filter_map(|author| author.as_str())
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let year = json_str(item, "first_publish_year");
            let editions = json_str(item, "edition_count");
            let mut content = String::new();
            if !author.is_empty() {
                content.push_str(&format!("by {author}. "));
            }
            if !year.is_empty() {
                content.push_str(&format!("First published in {year}. "));
            }
            if !editions.is_empty() {
                content.push_str(&format!("{editions} editions."));
            }
            let mut builder = ResultBuilder::new(format!("https://openlibrary.org{key}"), title)
                .content(content.trim().to_string())
                .template(crate::results::Template::Default);
            if !author.is_empty() {
                builder = builder.author(author);
            }
            if !year.is_empty() {
                builder = builder.published_date(year);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}
