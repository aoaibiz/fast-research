//! Springer Nature engine, ported from searx/engines/springer.py.
// engine-id: springer

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "springer",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

const BASE_URL: &str = "https://api.springernature.com/meta/v2/json";
const PAGE_SIZE: u32 = 10;

/// Springer Nature Meta API. Requires an API key.
pub struct Springer {
    meta: EngineMeta,
}

impl Springer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Springer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let key = self.meta.api_key.as_deref().unwrap_or_default();
        let pairs: Vec<(&str, String)> = vec![
            ("api_key", key.to_string()),
            ("q", query.query_text()),
            ("s", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
            ("p", PAGE_SIZE.to_string()),
        ];
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
        // A premium filter answers with 403 and a "premium feature" message;
        // upstream treats that as an empty result, not as an engine failure.
        if response.status == 403
            && json_str(&json, "status").eq_ignore_ascii_case("fail")
            && json_str(&json, "message")
                .to_lowercase()
                .contains("premium feature")
        {
            return Ok(Vec::new());
        }
        let items = match json_path(&json, "records").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for record in items {
            let title = clean(&json_str(record, "title"));
            let (html_url, pdf_url) = links(record);
            if title.is_empty() || !html_url.starts_with("http") {
                continue;
            }
            let authors: Vec<String> = json_path(record, "creators")
                .and_then(|value| value.as_array())
                .map(|creators| {
                    creators
                        .iter()
                        .map(|author| author_name(&json_str(author, "creator")))
                        .filter(|name| !name.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(html_url, title)
                    .content(strip_tags(&json_str(record, "abstract")))
                    .author(authors.join(", "))
                    .published_date(json_str(record, "publicationDate"))
                    .metadata("doi", json_str(record, "doi"))
                    .metadata("journal", json_str(record, "publicationName"))
                    .metadata("publisher", json_str(record, "publisher"))
                    .metadata("type", json_str(record, "contentType"))
                    .metadata("volume", json_str(record, "volume"))
                    .metadata("number", json_str(record, "number"))
                    .metadata("pages", pages(record))
                    .metadata("issn", json_str(record, "issn"))
                    .metadata("isbn", json_str(record, "isbn"))
                    .metadata("tags", tags(record))
                    .metadata("pdf_url", pdf_url)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream reverses the comma separated "Last, First" creator names.
fn author_name(value: &str) -> String {
    clean(&value.rsplit(", ").collect::<Vec<_>>().join(" "))
}

/// The web HTML and PDF links of a record, http is upgraded to https.
fn links(record: &serde_json::Value) -> (String, String) {
    let mut html = String::new();
    let mut pdf = String::new();
    let Some(urls) = json_path(record, "url").and_then(|value| value.as_array()) else {
        return (html, pdf);
    };
    for item in urls {
        if json_str(item, "platform") != "web" {
            continue;
        }
        let value = json_str(item, "value").replacen("http://", "https://", 1);
        match json_str(item, "format").as_str() {
            "html" => html = value,
            "pdf" => pdf = value,
            _ => {}
        }
    }
    (html, pdf)
}

fn pages(record: &serde_json::Value) -> String {
    let start = json_str(record, "startingPage");
    let end = json_str(record, "endingPage");
    if start.is_empty() {
        end
    } else if end.is_empty() {
        start
    } else {
        format!("{start}-{end}")
    }
}

fn tags(record: &serde_json::Value) -> String {
    json_path(record, "keyword")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records() {
        let body = r##"{"records":[{"title":"Rust study","abstract":"<jats:p>An abstract.</jats:p>","publicationDate":"2023-05-01T00:00:00","doi":"10.1/xyz","publicationName":"Journal","publisher":"Press","contentType":"article","volume":"5","number":"2","startingPage":"1","endingPage":"9","issn":"1234-5678","isbn":"978-1","keyword":["rust","systems"],"creators":[{"creator":"Doe, John"},{"creator":"Roe, Jane"}],"url":[{"platform":"web","format":"html","value":"http://example.com/a"},{"platform":"web","format":"pdf","value":"http://example.com/a.pdf"},{"platform":"mobile","format":"html","value":"http://m.example.com"}]}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let config = EngineConfig::new("springer nature", "springer", "springer", &["science"]);
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = Springer::from_config(&config)
            .unwrap()
            .parse(&response, &query)
            .unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/a");
        assert_eq!(result.title, "Rust study");
        assert_eq!(result.content, "An abstract.");
        assert_eq!(result.author.as_deref(), Some("John Doe, Jane Roe"));
        assert_eq!(
            result.published_date.as_deref(),
            Some("2023-05-01T00:00:00")
        );
        let expected = [
            ("doi", "10.1/xyz"),
            ("journal", "Journal"),
            ("publisher", "Press"),
            ("type", "article"),
            ("volume", "5"),
            ("number", "2"),
            ("pages", "1-9"),
            ("issn", "1234-5678"),
            ("isbn", "978-1"),
            ("tags", "rust, systems"),
            ("pdf_url", "https://example.com/a.pdf"),
        ];
        for (key, value) in expected {
            assert_eq!(result.metadata.get(key).map(String::as_str), Some(value));
        }
    }

    #[test]
    fn builds_the_paged_query() {
        let mut config = EngineConfig::new("springer nature", "springer", "springer", &["science"]);
        config.api_key = Some("abc123".to_string());
        let engine = Springer::from_config(&config).unwrap();
        let query = Query::parse("rust lang", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        for part in ["api_key=abc123", "q=rust%20lang", "s=10", "p=10"] {
            assert!(
                request.url.contains(part),
                "missing {part} in {}",
                request.url
            );
        }
    }
}
