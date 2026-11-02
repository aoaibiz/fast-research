//! Europe PMC engine, ported from searx/engines/europepmc.py.
// engine-id: europepmc

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "epmc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest/search";
const ARTICLE_URL: &str = "https://europepmc.org/article/";
const PAGE_SIZE: u32 = 20;
const MAX_PAGE_SIZE: u32 = 1000;

/// Europe PMC life sciences literature search.
pub struct EuropePmc {
    meta: EngineMeta,
}

impl EuropePmc {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for EuropePmc {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // Upstream stores the nextCursorMark the API returns in a cache and
        // sends it for the following pages. The frozen interface has no cache,
        // so a page is fetched together with all preceding ones and parse skips
        // them (the API has no plain page parameter).
        let base = self.meta.base_url.as_deref().unwrap_or(SEARCH_URL);
        let size = (PAGE_SIZE * pageno.max(1)).min(MAX_PAGE_SIZE);
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("format", "json".to_string()),
            ("resultType", "core".to_string()),
            ("pageSize", size.to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{base}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "resultList.result").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let skip = (query.pageno.max(1) as usize - 1) * PAGE_SIZE as usize;
        let mut results = Vec::new();
        for item in items.iter().skip(skip) {
            let source = json_str(item, "source");
            let identifier = json_str(item, "id");
            if source.is_empty() || identifier.is_empty() {
                continue;
            }
            let url = format!("{ARTICLE_URL}{source}/{identifier}");
            let title = strip_tags(&json_str(item, "title"));
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let types = json_path(item, "pubTypeList.pubType")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url.clone(), title)
                    .content(strip_tags(&json_str(item, "abstractText")))
                    .author(authors(item).join(", "))
                    .published_date(json_str(item, "firstPublicationDate"))
                    .metadata("doi", json_str(item, "doi"))
                    .metadata("journal", json_str(item, "journalInfo.journal.title"))
                    .metadata("issn", json_str(item, "journalInfo.journal.issn"))
                    .metadata("type", types)
                    .metadata("pdf_url", pdf_url(item))
                    .metadata("html_url", url)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream strips the trailing period of the comma separated author string.
fn authors(item: &serde_json::Value) -> Vec<String> {
    json_str(item, "authorString")
        .split(',')
        .map(str::trim)
        .map(|author| author.trim_end_matches('.').trim())
        .filter(|author| !author.is_empty())
        .map(str::to_string)
        .collect()
}

/// The open access PDF of the record, empty when there is none.
fn pdf_url(item: &serde_json::Value) -> String {
    let Some(urls) =
        json_path(item, "fullTextUrlList.fullTextUrl").and_then(|value| value.as_array())
    else {
        return String::new();
    };
    for info in urls {
        if json_str(info, "documentStyle") == "pdf" && json_str(info, "availabilityCode") == "OA" {
            return json_str(info, "url");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<EuropePmc> {
        EuropePmc::from_config(&EngineConfig::new(
            "europepmc",
            "europepmc",
            "epmc",
            &["science"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_result_list() {
        let body = r##"{"resultList":{"result":[
          {"id":"41710352","source":"MED","title":"A <i>Rust</i> paper",
           "authorString":"Messow V, Habeck M.","doi":"10.1/xyz",
           "firstPublicationDate":"2026-01-02","abstractText":"Plain abstract.",
           "pubTypeList":{"pubType":["Journal Article","brief-report"]},
           "journalInfo":{"journal":{"title":"Bioinformatics advances","issn":"2635-0041"}},
           "fullTextUrlList":{"fullTextUrl":[
             {"documentStyle":"pdf","availabilityCode":"OA","url":"https://example.com/a.pdf"}]}},
          {"id":"","source":"MED","title":"no id"},
          {"id":"1","source":"","title":"no source"}
        ]}}"##;
        let response = EngineResponse::new(200, "https://www.ebi.ac.uk/", body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://europepmc.org/article/MED/41710352");
                assert_eq!(result.title, "A Rust paper");
                assert_eq!(result.content, "Plain abstract.");
                assert_eq!(result.author.as_deref(), Some("Messow V, Habeck M"));
                assert_eq!(result.published_date.as_deref(), Some("2026-01-02"));
                assert_eq!(
                    result.metadata.get("pdf_url").map(String::as_str),
                    Some("https://example.com/a.pdf")
                );
                assert_eq!(
                    result.metadata.get("type").map(String::as_str),
                    Some("Journal Article, brief-report")
                );
                assert_eq!(
                    result.metadata.get("issn").map(String::as_str),
                    Some("2635-0041")
                );
                assert_eq!(
                    result.metadata.get("html_url").map(String::as_str),
                    Some("https://europepmc.org/article/MED/41710352")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn missing_result_list_is_empty() {
        let response = EngineResponse::new(200, SEARCH_URL, b"{}".to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn pages_with_a_larger_window() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("pageSize=40"));
        assert!(request.url.contains("resultType=core"));
        assert!(request.url.contains("query=rust"));
    }
}
