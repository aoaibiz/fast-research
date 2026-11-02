//! PubMed engine, ported from searx/engines/pubmed.py.
//!
//! Upstream runs two E-utilities calls (esearch for the PMIDs, efetch for the
//! records). The engine therefore uses the offline path and performs both
//! requests itself, so the result set matches upstream exactly.
// engine-id: pubmed

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::util::ResultBuilder;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "pub",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";
const PUBMED_URL: &str = "https://www.ncbi.nlm.nih.gov/pubmed/";
const PAGE_SIZE: u32 = 10;

/// PubMed biomedical literature search.
pub struct PubMed {
    meta: EngineMeta,
}

impl PubMed {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// esearch: the PMIDs of one result page.
    fn esearch_url(&self, query: &Query, pageno: u32) -> String {
        format!(
            "{EUTILS}/esearch.fcgi?db=pubmed&retmode=json&retmax={PAGE_SIZE}&retstart={}&term={}",
            (pageno.saturating_sub(1)) * PAGE_SIZE,
            crate::engine::util::encode(&query.query_text())
        )
    }
}

impl Engine for PubMed {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "pubmed performs two requests and runs as an offline engine".into(),
        ))
    }

    fn parse(
        &self,
        _response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        Ok(Vec::new())
    }

    fn is_offline(&self) -> bool {
        true
    }

    fn search_offline(&self, query: &Query, pageno: u32) -> Result<Vec<EngineItem>, EngineError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| EngineError::Message(format!("pubmed: runtime: {error}")))?;
        let timeout = self.meta.timeout.max(Duration::from_secs(2));
        let esearch = self.esearch_url(query, pageno);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(crate::network::default_user_agent())
            .build()
            .map_err(|error| EngineError::Message(format!("pubmed: client: {error}")))?;
        let efetch_xml = runtime.block_on(async move {
            let esearch_body = fetch(&client, &esearch).await?;
            let pmids = parse_ids(&esearch_body);
            if pmids.is_empty() {
                return Ok::<String, EngineError>(String::new());
            }
            let efetch = format!(
                "{EUTILS}/efetch.fcgi?db=pubmed&retmode=xml&id={}",
                pmids.join(",")
            );
            fetch(&client, &efetch).await
        })?;
        if efetch_xml.is_empty() {
            return Ok(Vec::new());
        }
        Ok(parse_records(&efetch_xml))
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<String, EngineError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| EngineError::Message(format!("pubmed: request failed: {error}")))?;
    if !response.status().is_success() {
        return Err(EngineError::Message(format!(
            "pubmed: http status {}",
            response.status().as_u16()
        )));
    }
    response
        .text()
        .await
        .map_err(|error| EngineError::Message(format!("pubmed: body: {error}")))
}

/// The PMIDs of an esearch JSON answer.
fn parse_ids(body: &str) -> Vec<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    document
        .get("esearchresult")
        .and_then(|result| result.get("idlist"))
        .and_then(|list| list.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One record of the efetch XML.
#[derive(Default)]
struct Record {
    pmid: String,
    title: String,
    abstract_text: String,
    journal: String,
    issn: String,
    doi: String,
    authors: Vec<String>,
    accepted: Option<String>,
}

/// Parse the efetch XML into results (upstream response()).
fn parse_records(xml: &str) -> Vec<EngineItem> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut records: Vec<Record> = Vec::new();
    let mut current: Option<Record> = None;
    let mut text = String::new();
    let mut fore_name = String::new();
    let mut last_name = String::new();
    let mut in_author = false;
    let mut accepted_depth = 0usize;
    let mut date_parts: BTreeMap<String, String> = BTreeMap::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.local_name().as_ref()).to_string();
                if name == "PubmedArticle" {
                    current = Some(Record::default());
                }
                if name == "Author" {
                    in_author = true;
                    fore_name.clear();
                    last_name.clear();
                }
                if name == "PubMedPubDate" {
                    let accepted = start.attributes().flatten().any(|attribute| {
                        attribute.key.as_ref() == b"PubStatus"
                            && attribute.value.as_ref() == b"accepted"
                    });
                    if accepted {
                        accepted_depth = stack.len() + 1;
                        date_parts.clear();
                    }
                }
                text.clear();
                stack.push(name);
            }
            Ok(Event::Text(value)) => {
                text.push_str(&String::from_utf8_lossy(&value.into_inner()));
            }
            Ok(Event::CData(value)) => {
                text.push_str(&String::from_utf8_lossy(&value.into_inner()));
            }
            Ok(Event::End(end)) => {
                let name = String::from_utf8_lossy(end.local_name().as_ref()).to_string();
                stack.pop();
                let value = text.trim().to_string();
                if let Some(record) = current.as_mut() {
                    match name.as_str() {
                        "PMID" if record.pmid.is_empty() => record.pmid = value.clone(),
                        "ArticleTitle" => record.title = value.clone(),
                        "AbstractText" => {
                            if !record.abstract_text.is_empty() {
                                record.abstract_text.push(' ');
                            }
                            record.abstract_text.push_str(&value);
                        }
                        "Title" if stack.iter().any(|entry| entry == "Journal") => {
                            record.journal = value.clone()
                        }
                        "ISSN" => record.issn = value.clone(),
                        "ELocationID" if stack.iter().any(|entry| entry == "MedlineCitation") => {
                            if record.doi.is_empty() {
                                record.doi = value.clone();
                            }
                        }
                        "ForeName" if in_author => fore_name = value.clone(),
                        "LastName" if in_author => last_name = value.clone(),
                        "Author" => {
                            let author = format!("{fore_name} {last_name}").trim().to_string();
                            if !author.is_empty() {
                                record.authors.push(author);
                            }
                            in_author = false;
                        }
                        "Year"
                            if accepted_depth > 0
                                && stack.last().is_some_and(|p| p == "PubMedPubDate") =>
                        {
                            date_parts.insert("year".into(), value.clone());
                        }
                        "Month"
                            if accepted_depth > 0
                                && stack.last().is_some_and(|p| p == "PubMedPubDate") =>
                        {
                            date_parts.insert("month".into(), value.clone());
                        }
                        "Day"
                            if accepted_depth > 0
                                && stack.last().is_some_and(|p| p == "PubMedPubDate") =>
                        {
                            date_parts.insert("day".into(), value.clone());
                        }
                        "PubMedPubDate" if accepted_depth > 0 => {
                            if let (Some(year), Some(month), Some(day)) = (
                                date_parts.get("year").cloned(),
                                date_parts.get("month").cloned(),
                                date_parts.get("day").cloned(),
                            ) {
                                if let (Ok(year), Ok(month), Ok(day)) = (
                                    year.parse::<i32>(),
                                    month.parse::<u32>(),
                                    day.parse::<u32>(),
                                ) {
                                    record.accepted =
                                        Some(format!("{year:04}-{month:02}-{day:02}"));
                                }
                            }
                            accepted_depth = 0;
                        }
                        _ => {}
                    }
                }
                if name == "PubmedArticle" {
                    if let Some(record) = current.take() {
                        records.push(record);
                    }
                }
                text.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    records
        .into_iter()
        .filter(|record| !record.pmid.is_empty() && !record.title.is_empty())
        .map(|record| {
            let url = format!("{PUBMED_URL}{}", record.pmid);
            let mut builder = ResultBuilder::new(url, record.title)
                .content(record.abstract_text)
                .author(record.authors.join(", "))
                .metadata("journal", record.journal)
                .metadata("issn", record.issn)
                .metadata("doi", record.doi);
            if let Some(date) = record.accepted {
                builder = builder.published_date(date);
            }
            builder.build()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESEARCH: &str = r##"{"esearchresult":{"idlist":["111","222"]}}"##;
    const EFETCH: &str = r##"<PubmedArticleSet>
      <PubmedArticle>
        <MedlineCitation>
          <PMID>111</PMID>
          <Article>
            <ArticleTitle>A study of rust</ArticleTitle>
            <Abstract><AbstractText>An abstract.</AbstractText></Abstract>
            <Journal><Title>Journal of Rust</Title><ISSN>1234-5678</ISSN></Journal>
            <AuthorList><Author><ForeName>Ada</ForeName><LastName>Lovelace</LastName></Author></AuthorList>
            <ELocationID EIdType="doi">10.1000/xyz</ELocationID>
          </Article>
        </MedlineCitation>
        <PubmedData><History><PubMedPubDate PubStatus="accepted"><Year>2024</Year><Month>5</Month><Day>1</Day></PubMedPubDate></History></PubmedData>
      </PubmedArticle>
    </PubmedArticleSet>"##;

    #[test]
    fn esearch_ids_are_read() {
        assert_eq!(
            parse_ids(ESEARCH),
            vec!["111".to_string(), "222".to_string()]
        );
        assert!(parse_ids("<html></html>").is_empty());
    }

    #[test]
    fn efetch_records_become_results() {
        let items = parse_records(EFETCH);
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ncbi.nlm.nih.gov/pubmed/111");
                assert_eq!(result.title, "A study of rust");
                assert_eq!(result.content, "An abstract.");
                assert_eq!(result.author.as_deref(), Some("Ada Lovelace"));
                assert_eq!(
                    result.metadata.get("journal").map(String::as_str),
                    Some("Journal of Rust")
                );
                assert_eq!(
                    result.metadata.get("doi").map(String::as_str),
                    Some("10.1000/xyz")
                );
                assert_eq!(result.published_date.as_deref(), Some("2024-05-01"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn the_engine_runs_offline() {
        let config = EngineConfig::new("pubmed", "pubmed", "pub", &["science"]);
        let engine = PubMed::from_config(&config).expect("engine");
        assert!(engine.is_offline());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine.esearch_url(&query, 2).contains("retstart=10"));
    }
}
