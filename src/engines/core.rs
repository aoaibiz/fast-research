//! CORE engine, ported from `searx/engines/core.py`.

// engine-id: core

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "core",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.core.ac.uk/v3/search/works/";
const PAGE_SIZE: u32 = 10;

/// CORE (COnnecting REpositories) bibliographic search. Requires an API key.
pub struct Core {
    meta: EngineMeta,
}

impl Core {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Core {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            (
                "offset",
                ((pageno.saturating_sub(1)) * PAGE_SIZE).to_string(),
            ),
            ("limit", PAGE_SIZE.to_string()),
            ("sort", "relevance".to_string()),
        ];
        let mut request = EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)));
        if let Some(key) = self.meta.api_key.as_deref() {
            if !key.is_empty() {
                request.set_header("Authorization", &format!("Bearer {key}"));
            }
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "results").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            // Choose the target URL following upstream's priority chain.
            let url = {
                let doi = json_str(item, "doi");
                if !doi.is_empty() {
                    format!("https://doi.org/{doi}")
                } else {
                    let id = json_str(item, "id");
                    if !id.is_empty() {
                        format!("https://core.ac.uk/works/{id}")
                    } else {
                        let download = json_str(item, "downloadUrl");
                        if !download.is_empty() {
                            download
                        } else {
                            let fulltext = json_str(item, "sourceFulltextUrls");
                            if fulltext.is_empty() {
                                continue;
                            }
                            fulltext
                        }
                    }
                }
            };
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let raw_date = json_str(item, "publishedDate");
            let published_date = if raw_date.is_empty() {
                json_str(item, "depositedDate")
            } else {
                raw_date.replace('Z', "+00:00")
            };
            let content = json_str(item, "fullText");
            let publishers: String = json_path(item, "journals")
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|j| j.get("title").and_then(|t| t.as_str()).map(str::to_string))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let authors: Vec<String> = json_path(item, "authors")
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let doc_type = json_str(item, "documentType");
            let tag = json_str(item, "fieldOfStudy");
            let mut builder = ResultBuilder::new(url, title).content(content);
            if !published_date.is_empty() {
                builder = builder.published_date(published_date);
            }
            if !publishers.is_empty() {
                builder = builder.metadata("journal", publishers);
            }
            if !authors.is_empty() {
                builder = builder.author(authors.join(", "));
            }
            if !doc_type.is_empty() {
                builder = builder.metadata("type", doc_type);
            }
            if !tag.is_empty() {
                builder = builder.metadata("tags", tag);
            }
            let doi = json_str(item, "doi");
            if !doi.is_empty() {
                builder = builder.metadata("doi", doi);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_core_works() {
        let body = r#"{"results":[
          {"title":"A Paper","id":"12345","doi":"10.1000/xyz","fullText":"Abstract here.",
           "publishedDate":"2023-05-01T00:00:00Z","documentType":"research article",
           "fieldOfStudy":"Computer Science",
           "authors":[{"name":"Alice"},{"name":"Bob"}],
           "journals":[{"title":"Journal A"}]}
        ]}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = Core::from_config(&EngineConfig::new(
            "core.ac.uk",
            "core",
            "core",
            &["science"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("ai", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://doi.org/10.1000/xyz");
                assert_eq!(result.title, "A Paper");
                assert_eq!(result.author.as_deref(), Some("Alice, Bob"));
            }
            _ => panic!("expected a result"),
        }
    }
}
