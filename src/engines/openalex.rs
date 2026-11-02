//! OpenAlex engine, ported from searx/engines/openalex.py.
// engine-id: openalex

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "oa",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

const SEARCH_URL: &str = "https://api.openalex.org/works";
const PAGE_SIZE: u32 = 10;

/// OpenAlex scholarly works search.
pub struct OpenAlex {
    meta: EngineMeta,
}

impl OpenAlex {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for OpenAlex {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(SEARCH_URL);
        let mut pairs: Vec<(&str, String)> = vec![
            ("search", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("per-page", PAGE_SIZE.to_string()),
            ("sort", "relevance_score:desc".to_string()),
        ];
        // Upstream filters by the ISO639-1 language of the request, the
        // OpenAlex language filter only accepts two letter codes.
        let language = query.engine_language();
        if language != "all" {
            let iso2 = language.split(['-', '_']).next().unwrap_or_default();
            if iso2.len() == 2 {
                pairs.push(("filter", format!("language:{iso2}")));
            }
        }
        if let Some(mailto) = self.meta.opt_str("mailto") {
            if !mailto.is_empty() {
                pairs.push(("mailto", mailto.to_string()));
            }
        }
        Ok(
            EngineRequest::get(format!("{base}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
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
            let title = clean(&json_str(item, "title"));
            let mut url = json_str(item, "primary_location.landing_page_url");
            if url.is_empty() {
                url = json_str(item, "id");
            }
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let biblio = json_path(item, "biblio");
            let volume = biblio
                .map(|value| json_str(value, "volume"))
                .unwrap_or_default();
            let number = biblio
                .map(|value| json_str(value, "issue"))
                .unwrap_or_default();
            let doi = json_str(item, "doi");
            let doi = doi
                .strip_prefix("https://doi.org/")
                .unwrap_or(&doi)
                .to_string();
            let comments = json_path(item, "cited_by_count")
                .and_then(|value| value.as_i64())
                .map(|count| format!("{count} citations"))
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(reconstruct_abstract(json_path(
                        item,
                        "abstract_inverted_index",
                    )))
                    .author(names(item, "authorships", "author.display_name").join(", "))
                    .published_date(
                        parse_date(&json_str(item, "publication_date")).unwrap_or_default(),
                    )
                    .metadata("doi", doi)
                    .metadata(
                        "journal",
                        first_value(
                            item,
                            &[
                                "host_venue.display_name",
                                "primary_location.source.display_name",
                            ],
                        ),
                    )
                    .metadata(
                        "publisher",
                        first_value(
                            item,
                            &[
                                "host_venue.publisher",
                                "primary_location.source.host_organization_name",
                            ],
                        ),
                    )
                    .metadata("type", json_str(item, "type"))
                    .metadata("tags", names(item, "concepts", "display_name").join(", "))
                    .metadata(
                        "pdf_url",
                        first_value(item, &["primary_location.pdf_url", "open_access.oa_url"]),
                    )
                    .metadata("pages", biblio.map(pages).unwrap_or_default())
                    .metadata("volume", volume)
                    .metadata("number", number)
                    .metadata("comments", comments)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream rebuilds the abstract from the inverted index of OpenAlex.
fn reconstruct_abstract(value: Option<&serde_json::Value>) -> String {
    let Some(index) = value.and_then(|value| value.as_object()) else {
        return String::new();
    };
    let mut tokens: Vec<(u64, &str)> = Vec::new();
    for (token, positions) in index {
        let Some(positions) = positions.as_array() else {
            continue;
        };
        for position in positions {
            if let Some(position) = position.as_u64() {
                tokens.push((position, token.as_str()));
            }
        }
    }
    tokens.sort_by_key(|(position, _)| *position);
    tokens
        .into_iter()
        .map(|(_, token)| token)
        .collect::<Vec<_>>()
        .join(" ")
}

/// OpenAlex may return YYYY, YYYY-MM or YYYY-MM-DD; upstream parses all three.
fn parse_date(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split('-').collect();
    match parts.as_slice() {
        [year] if year.len() == 4 => Some(format!("{year}-01-01")),
        [year, month] if year.len() == 4 && month.len() == 2 => Some(format!("{year}-{month}-01")),
        [year, month, day] if year.len() == 4 && month.len() == 2 && day.len() == 2 => {
            Some(value.to_string())
        }
        _ => None,
    }
}

/// First and last page of the biblio block.
fn pages(biblio: &serde_json::Value) -> String {
    let first = json_str(biblio, "first_page");
    let last = json_str(biblio, "last_page");
    if first.is_empty() {
        last
    } else if last.is_empty() {
        first
    } else {
        format!("{first}-{last}")
    }
}

/// The first non empty JSON string among the given paths.
fn first_value(item: &serde_json::Value, paths: &[&str]) -> String {
    paths
        .iter()
        .map(|path| json_str(item, path))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// The field of every object of the array at the given key.
fn names(item: &serde_json::Value, key: &str, field: &str) -> Vec<String> {
    json_path(item, key)
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .map(|entry| json_str(entry, field))
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<OpenAlex> {
        let mut config = EngineConfig::new("openalex", "openalex", "oa", &["science"]);
        config.extra.insert(
            "mailto".into(),
            toml::Value::String("oa@example.com".into()),
        );
        OpenAlex::from_config(&config).unwrap()
    }

    #[test]
    fn parses_works_and_rebuilds_abstracts() {
        let body = r##"{"results":[{"id":"https://openalex.org/W1","title":"Rust for systems","doi":"https://doi.org/10.1000/xyz","type":"article","publication_date":"2023-05","cited_by_count":7,"abstract_inverted_index":{"Rust":[0],"is":[1],"fast":[2]},"primary_location":{"landing_page_url":"https://example.com/paper","pdf_url":"https://example.com/paper.pdf","source":{"display_name":"Journal of Rust","host_organization_name":"Rust Press"}},"host_venue":null,"biblio":{"volume":"3","issue":"2","first_page":"10","last_page":"20"},"authorships":[{"author":{"display_name":"Alice"}},{"author":{"display_name":"Bob"}}],"concepts":[{"display_name":"Computer science"}]},{"id":"https://openalex.org/W2","title":""}]}"##;
        let response = EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/paper");
        assert_eq!(result.content, "Rust is fast");
        assert_eq!(result.author.as_deref(), Some("Alice, Bob"));
        assert_eq!(result.published_date.as_deref(), Some("2023-05-01"));
        let expected = [
            ("doi", "10.1000/xyz"),
            ("journal", "Journal of Rust"),
            ("publisher", "Rust Press"),
            ("pages", "10-20"),
            ("volume", "3"),
            ("number", "2"),
            ("comments", "7 citations"),
            ("tags", "Computer science"),
            ("pdf_url", "https://example.com/paper.pdf"),
        ];
        for (key, value) in expected {
            assert_eq!(result.metadata.get(key).map(String::as_str), Some(value));
        }
    }

    #[test]
    fn filters_by_language_and_sends_mailto() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.languages = vec!["fr".to_string()];
        let request = engine().build_request(&query, 1).unwrap();
        let parts = [
            "search=rust",
            "page=1",
            "per-page=10",
            "sort=relevance_score%3Adesc",
            "filter=language%3Afr",
            "mailto=oa%40example.com",
        ];
        for part in parts {
            assert!(
                request.url.contains(part),
                "missing {part} in {}",
                request.url
            );
        }
    }
}
