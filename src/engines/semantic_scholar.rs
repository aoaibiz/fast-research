//! Semantic Scholar engine, ported from `searx/engines/semantic_scholar.py`.
//!
//! Upstream scrapes the website's internal POST API and needs a cached
//! X-S2-UI-Version header. This port uses the key-free public Graph API
//! instead, which returns the same paper metadata without a key.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "se",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.semanticscholar.org/graph/v1/paper/search";
const FIELDS: &str = "title,url,abstract,authors,year,venue,externalIds";
const PAGE_SIZE: u32 = 10;

/// Semantic Scholar's public Graph API.
pub struct SemanticScholar {
    meta: EngineMeta,
}

impl SemanticScholar {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for SemanticScholar {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("query", query.query_text()),
            ("fields", FIELDS.to_string()),
            ("limit", PAGE_SIZE.to_string()),
            ("offset", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            let url = paper_url(item);
            if !is_http_url(&url) {
                continue;
            }
            let authors = json_path(item, "authors")
                .and_then(serde_json::Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|author| json_str(author, "name"))
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(item, "abstract")))
                    .author(authors)
                    .published_date(json_str(item, "year"))
                    .metadata("venue", json_str(item, "venue"))
                    .metadata("doi", json_str(item, "externalIds.DOI"))
                    .metadata("paper_id", json_str(item, "paperId"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The paper URL, falling back to the DOI and finally to the Semantic Scholar
/// landing page for the paper id.
fn paper_url(item: &serde_json::Value) -> String {
    let url = json_str(item, "url");
    if is_http_url(&url) {
        return url;
    }
    let doi = json_str(item, "externalIds.DOI");
    if !doi.is_empty() {
        return format!("https://doi.org/{doi}");
    }
    let paper_id = json_str(item, "paperId");
    if paper_id.is_empty() {
        String::new()
    } else {
        format!("https://www.semanticscholar.org/paper/{paper_id}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "total": 1,
      "offset": 0,
      "data": [
        {
          "paperId": "abc123",
          "title": "Deep Learning",
          "url": "https://www.semanticscholar.org/paper/abc123",
          "abstract": "A survey of deep learning.",
          "year": 2015,
          "venue": "Nature",
          "externalIds": {"DOI": "10.1038/nature14539"},
          "authors": [{"authorId": "1", "name": "Yann LeCun"}, {"authorId": "2", "name": "Yoshua Bengio"}]
        }
      ]
    }"#;

    fn engine() -> Arc<SemanticScholar> {
        SemanticScholar::from_config(&EngineConfig::new(
            "semanticscholar",
            "semantic_scholar",
            "se",
            &["science"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_graph_api_response() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse(
            "deep learning",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Deep Learning");
                assert_eq!(result.url, "https://www.semanticscholar.org/paper/abc123");
                assert_eq!(result.author.as_deref(), Some("Yann LeCun, Yoshua Bengio"));
                assert_eq!(result.published_date.as_deref(), Some("2015"));
                assert_eq!(
                    result.metadata.get("doi").map(String::as_str),
                    Some("10.1038/nature14539")
                );
            }
            _ => panic!("expected a result"),
        }
    }
}
