//! Astrophysics Data System engine, ported from searx/engines/astrophysics_data_system.py.
//!
//! The API key is mandatory: upstream disables the engine without a real key,
//! and this port refuses to build in the same case.

// engine-id: astrophysics_data_system

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "ads",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.adsabs.harvard.edu/v1/search/query";
const PAPER_URL: &str = "https://ui.adsabs.harvard.edu/abs";
const PAGE_SIZE: i64 = 10;
const MAX_AUTHORS: usize = 15;
const PLACEHOLDERS: &[&str] = &["", "unset", "unknown", "..."];
const FIELDS: &str = "abstract,author,bibcode,comment,date,doi,isbn,issn,keyword,page,page_count,page_range,pub,pubdate,pubnote,read_count,title,volume,year";

/// The ADS paper search.
pub struct AstrophysicsDataSystem {
    meta: EngineMeta,
}

impl AstrophysicsDataSystem {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        match meta.api_key.as_deref().map(str::trim) {
            Some(key) if !PLACEHOLDERS.contains(&key) => Ok(Arc::new(Self { meta })),
            _ => Err("astrophysics_data_system requires an api_key".to_string()),
        }
    }
}

impl Engine for AstrophysicsDataSystem {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.meta.api_key.as_deref();
        let key = key.map(str::trim).unwrap_or_default();
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        let sort = self.meta.opt_str("ads_sort");
        let sort = sort.unwrap_or("read_count desc");
        let start = (pageno.max(1) - 1) as i64 * PAGE_SIZE;
        let pairs = vec![
            ("q", query.query_text()),
            ("fl", FIELDS.to_string()),
            ("rows", PAGE_SIZE.to_string()),
            ("start", start.to_string()),
            ("sort", sort.to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{base}?{}", query_string(&pairs)))
                .header("Authorization", &format!("Bearer {key}"))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(error) = json_path(&json, "error") {
            return Err(EngineError::Message(json_str(error, "msg")));
        }
        let docs = match json_path(&json, "response.docs").and_then(|value| value.as_array()) {
            Some(docs) => docs,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for doc in docs {
            let title = strip_tags(&json_str(doc, "title.0"));
            let url = format!("{PAPER_URL}/{}/", json_str(doc, "bibcode"));
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut authors = string_list(doc, "author");
            if authors.len() > MAX_AUTHORS {
                authors.truncate(MAX_AUTHORS);
                authors.push("et al.".to_string());
            }
            let publisher = format!("{} {}", json_str(doc, "pub"), json_str(doc, "year"));
            results.push(
                ResultBuilder::new(url, title)
                    .content(strip_tags(&json_str(doc, "abstract")))
                    .author(authors.join(", "))
                    .published_date(json_str(doc, "date"))
                    .metadata("doi", string_list(doc, "doi").join(", "))
                    .metadata("issn", string_list(doc, "issn").join(", "))
                    .metadata("isbn", string_list(doc, "isbn").join(", "))
                    .metadata("tags", string_list(doc, "keyword").join(", "))
                    .metadata("pages", string_list(doc, "page").join(","))
                    .metadata("publisher", publisher.trim())
                    .metadata("volume", json_str(doc, "volume"))
                    .metadata("views", json_str(doc, "read_count"))
                    .metadata("comments", string_list(doc, "pubnote").join(" / "))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn string_list(value: &serde_json::Value, key: &str) -> Vec<String> {
    let Some(entries) = json_path(value, key).and_then(|entry| entry.as_array()) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for entry in entries {
        if let Some(value) = entry.as_str() {
            values.push(value.to_string());
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"{"response":{"docs":[
      {"bibcode":"2024ApJ...0001A","title":["A <b>Bright</b> Star"],"abstract":"We study stars.",
       "date":"2024-01-01T00:00:00Z","author":["A1","A2","A3","A4","A5","A6","A7","A8","A9","A10",
       "A11","A12","A13","A14","A15","A16"],"doi":["10.1000/xyz"],"issn":["0004-637X"],
       "keyword":["stars"],"page":["1","2"],"pub":"ApJ","year":"2024","volume":"100",
       "read_count":"42","pubnote":["Accepted"]}]}}"##;

    fn engine() -> Arc<AstrophysicsDataSystem> {
        let mut config = EngineConfig::new("ads", "astrophysics_data_system", "ads", &["science"]);
        config.api_key = Some("secret".to_string());
        AstrophysicsDataSystem::from_config(&config).unwrap()
    }

    fn search() -> Query {
        Query::parse("stars", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_paged_request() {
        let config = EngineConfig::new("ads", "astrophysics_data_system", "ads", &["science"]);
        assert!(AstrophysicsDataSystem::from_config(&config).is_err());
        let request = engine().build_request(&search(), 2).unwrap();
        assert!(request.url.starts_with(BASE_URL));
        assert!(request.url.contains("start=10"));
        assert!(request.url.contains("rows=10"));
        assert!(request.url.contains("q=stars"));
        assert!(request.url.contains("sort=read_count%20desc"));
        let authorization = request.headers.get("authorization").unwrap();
        assert_eq!(authorization.to_str().unwrap(), "Bearer secret");
    }

    #[test]
    fn parses_documents() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "A Bright Star");
        assert_eq!(
            result.url,
            "https://ui.adsabs.harvard.edu/abs/2024ApJ...0001A/"
        );
        assert_eq!(result.content, "We study stars.");
        assert!(
            result
                .author
                .as_deref()
                .unwrap_or_default()
                .ends_with("et al.")
        );
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-01-01T00:00:00Z")
        );
        assert_eq!(
            result.metadata.get("doi").map(String::as_str),
            Some("10.1000/xyz")
        );
        assert_eq!(
            result.metadata.get("pages").map(String::as_str),
            Some("1,2")
        );
        let publisher = result.metadata.get("publisher").map(String::as_str);
        assert_eq!(publisher, Some("ApJ 2024"));
        let empty = EngineResponse::new(200, BASE_URL, b"{\"response\":{}}".to_vec());
        assert!(engine().parse(&empty, &search()).unwrap().is_empty());
    }
}
