//! Open Semantic Search engine, ported from searx/engines/opensemantic.py.
//!
//! The engine queries the Solr endpoint that ships with Open Semantic Search.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: opensemantic

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

/// An Open Semantic Search instance.
pub struct OpenSemantic {
    meta: EngineMeta,
}

impl OpenSemantic {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for OpenSemantic {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or("http://localhost:8983/solr/opensemanticsearch/")
            .trim_end_matches('/');
        let rows = 10;
        let start = (pageno.saturating_sub(1)) * rows;
        Ok(EngineRequest::get(format!(
            "{base}/query?{}",
            query_string(&[
                ("q", query.query_text().to_string()),
                ("rows", rows.to_string()),
                ("start", start.to_string()),
                ("wt", "json".to_string()),
            ])
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
            let title = ["title", "name"]
                .iter()
                .map(|key| json_str(document, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            if !url.starts_with("http") || title.is_empty() {
                continue;
            }
            let content = ["content", "text", "description"]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<OpenSemantic> {
        let mut config = EngineConfig::new("opensemantic", "opensemantic", "", &["general"]);
        config.base_url = Some("http://localhost:8983/solr/opensemanticsearch/".to_string());
        OpenSemantic::from_config(&config).expect("engine")
    }

    #[test]
    fn queries_the_solr_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("/query?"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("start=10"));
    }

    #[test]
    fn parses_the_documents() {
        let body = r##"{"response":{"docs":[
            {"url":"https://example.test/a","title":"Title A","content":"Snippet A"},
            {"url":"","title":"no url"}
        ]}}"##;
        let response = EngineResponse::new(200, "http://localhost:8983", body.as_bytes().to_vec());
        let query = Query::parse("a", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/a");
                assert_eq!(result.title, "Title A");
            }
            _ => panic!("expected a result"),
        }
    }
}
