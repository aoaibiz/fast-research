//! Solr engine, ported from searx/engines/solr.py.
//!
//! Solr is a self-hosted Lucene backend; the engine queries one collection of
//! an instance configured with base_url and collection.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: solr

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "slr",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// A Solr collection.
pub struct Solr {
    meta: EngineMeta,
    collection: String,
    field_list: String,
    query_fields: String,
    default_fields: String,
    sort: String,
}

impl Solr {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let collection = meta.opt_str("collection").unwrap_or_default().to_string();
        if collection.is_empty() {
            return Err("solr requires the collection option".into());
        }
        let field_list = meta.opt_str("field_list").unwrap_or("name").to_string();
        let query_fields = meta.opt_str("query_fields").unwrap_or("").to_string();
        let default_fields = meta.opt_str("default_fields").unwrap_or("").to_string();
        let sort = meta.opt_str("sort").unwrap_or("").to_string();
        Ok(Arc::new(Self {
            meta,
            collection,
            field_list,
            query_fields,
            default_fields,
            sort,
        }))
    }
}

impl Engine for Solr {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .ok_or_else(|| EngineError::Message("solr requires base_url".into()))?
            .trim_end_matches('/');
        let mut pairs = vec![
            ("q", query.query_text().to_string()),
            ("rows", "10".to_string()),
            ("start", (pageno.saturating_sub(1) * 10).to_string()),
        ];
        if !self.field_list.is_empty() {
            pairs.push(("fl", self.field_list.clone()));
        }
        if !self.query_fields.is_empty() {
            pairs.push(("qf", self.query_fields.clone()));
        }
        if !self.default_fields.is_empty() {
            pairs.push(("df", self.default_fields.clone()));
        }
        if !self.sort.is_empty() {
            pairs.push(("sort", self.sort.clone()));
        }
        Ok(EngineRequest::get(format!(
            "{base}/solr/{}/select?{}",
            self.collection,
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(documents) =
            json_path(&json, "response.docs").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for document in documents {
            let url = ["url", "link", "id"]
                .iter()
                .map(|key| json_str(document, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let title = ["name", "title"]
                .iter()
                .map(|key| json_str(document, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let content = ["content", "description", "text"]
                .iter()
                .map(|key| json_str(document, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, strip_tags(&title))
                    .content(strip_tags(&content))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Solr> {
        let mut config = EngineConfig::new("solr", "solr", "slr", &["general"]);
        config.base_url = Some("http://localhost:8983".to_string());
        config
            .extra
            .insert("collection".into(), toml::Value::String("pages".into()));
        Solr::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_a_collection() {
        let config = EngineConfig::new("solr", "solr", "slr", &["general"]);
        assert!(Solr::from_config(&config).is_err());
    }

    #[test]
    fn queries_the_collection() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("http://localhost:8983/solr/pages/select?")
        );
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("start=10"));
    }

    #[test]
    fn parses_the_documents() {
        let body = r##"{"response":{"docs":[
            {"url":"https://example.test/a","name":"A","content":"About A"},
            {"id":"doc-1","name":"no url"}
        ]}}"##;
        let response = EngineResponse::new(200, "http://localhost:8983", body.as_bytes().to_vec());
        let query = Query::parse("a", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/a");
                assert_eq!(result.title, "A");
            }
            _ => panic!("expected a result"),
        }
    }
}
