//! Crossref engine, ported from `searx/engines/crossref.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "cr",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://api.crossref.org/works";
const PAGE_SIZE: u32 = 20;

/// Crossref's public works API.
pub struct Crossref {
    meta: EngineMeta,
}

impl Crossref {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Crossref {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs = vec![
            ("query", query.query_text()),
            ("offset", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
        ];
        if let Some(since) = crate::query::TimeRange::since(query.time_range) {
            // Newest first, restricted to the requested window.
            pairs.push(("sort", "published".to_string()));
            pairs.push(("order", "desc".to_string()));
            pairs.push((
                "filter",
                format!("from-pub-date:{}", since.format("%Y-%m-%d")),
            ));
        }
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
        let Some(items) = json_path(&json, "message.items").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for record in items {
            let kind = json_str(record, "type");
            // Files published along with a paper, not something you search for.
            if kind == "component" {
                continue;
            }
            let container = json_str(record, "container-title.0");
            let mut journal = String::new();
            let mut title = json_str(record, "title.0");
            if kind == "book-chapter" {
                title = container.clone();
                let chapter = json_str(record, "title.0");
                if !chapter.is_empty() && !chapter.eq_ignore_ascii_case(container.trim()) {
                    if title.is_empty() {
                        title = format!("({chapter})");
                    } else {
                        title = format!("{title} ({chapter})");
                    }
                }
            } else if title.is_empty() {
                title = container;
            } else {
                journal = container;
            }
            if title.is_empty() {
                continue;
            }
            let mut url = json_str(record, "URL");
            if let Some(primary) =
                json_path(record, "resource.primary.URL").and_then(|value| value.as_str())
            {
                if !primary.is_empty() {
                    url = primary.to_string();
                }
            }
            if !is_http_url(&url) {
                continue;
            }
            let authors = string_array(record, "author")
                .map(|entries| {
                    entries
                        .iter()
                        .map(|author| {
                            clean(&format!(
                                "{} {}",
                                json_str(author, "given"),
                                json_str(author, "family")
                            ))
                        })
                        .filter(|name| !name.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let tags = string_array(record, "subject")
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, clean(&title))
                    .content(strip_tags(&json_str(record, "abstract")))
                    .author(authors)
                    .published_date(published_date(record))
                    .metadata("doi", json_str(record, "DOI"))
                    .metadata("journal", journal)
                    .metadata("pages", json_str(record, "page"))
                    .metadata("publisher", json_str(record, "publisher"))
                    .metadata("type", kind)
                    .metadata("volume", json_str(record, "volume"))
                    .metadata("tags", tags)
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn string_array<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a Vec<serde_json::Value>> {
    json_path(value, key).and_then(serde_json::Value::as_array)
}

/// The published date, from the first date-parts entry, as YYYY-MM-DD.
fn published_date(record: &serde_json::Value) -> String {
    let Some(parts) =
        json_path(record, "published.date-parts.0").and_then(serde_json::Value::as_array)
    else {
        return String::new();
    };
    let mut values = parts.iter().filter_map(serde_json::Value::as_u64);
    let Some(year) = values.next() else {
        return String::new();
    };
    let month = values.next().unwrap_or(1);
    let day = values.next().unwrap_or(1);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "status": "ok",
      "message": {
        "items": [
          {
            "type": "journal-article",
            "title": ["Rust: a safe systems language"],
            "container-title": ["Journal of Systems"],
            "abstract": "<jats:p>An abstract.</jats:p>",
            "DOI": "10.1000/rust",
            "URL": "https://doi.org/10.1000/rust",
            "page": "1-10",
            "volume": "12",
            "publisher": "ACM",
            "published": {"date-parts": [[2023, 5, 17]]},
            "author": [{"given": "Jane", "family": "Doe"}, {"given": "John", "family": "Roe"}],
            "subject": ["Computer Science"]
          },
          {"type": "component", "title": ["Skipped"], "URL": "https://example.com/skipped"},
          {
            "type": "book-chapter",
            "title": ["Chapter One"],
            "container-title": ["A Book"],
            "URL": "https://example.com/book"
          }
        ]
      }
    }"#;

    fn engine() -> Arc<Crossref> {
        Crossref::from_config(&EngineConfig::new(
            "crossref",
            "crossref",
            "cr",
            &["science"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_journal_articles() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust: a safe systems language");
                assert_eq!(result.url, "https://doi.org/10.1000/rust");
                assert_eq!(result.author.as_deref(), Some("Jane Doe, John Roe"));
                assert_eq!(result.published_date.as_deref(), Some("2023-05-17"));
                assert_eq!(result.content, "An abstract.");
                assert_eq!(
                    result.metadata.get("journal").map(String::as_str),
                    Some("Journal of Systems")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.title, "A Book (Chapter One)"),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_offset_url() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with(SEARCH_URL));
        assert!(request.url.contains("offset=40"));
    }
}
