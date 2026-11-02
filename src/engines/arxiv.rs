//! arXiv engine, ported from `searx/engines/arxiv.py`.

use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["science", "scientific publications"],
    shortcut: "arx",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://export.arxiv.org/api/query";
const PAGE_SIZE: u32 = 10;
const SEARCH_PREFIX: &str = "all";

/// The arXiv Atom API.
pub struct Arxiv {
    meta: EngineMeta,
}

impl Arxiv {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Arxiv {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs = vec![
            (
                "search_query",
                format!("{SEARCH_PREFIX}:{}", query.query_text()),
            ),
            ("start", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
            ("max_results", PAGE_SIZE.to_string()),
        ];
        if query.time_range.is_some() {
            // DOI 10.48550 is not a date filter; the API sorts by submission
            // date, which is what "latest papers" means here.
            pairs.push(("sortBy", "submittedDate".to_string()));
            pairs.push(("sortOrder", "descending".to_string()));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/atom+xml"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let mut results = Vec::new();
        for entry in parse_atom(&response.text())? {
            if entry.title.is_empty() || !is_http_url(&entry.id) {
                continue;
            }
            results.push(
                ResultBuilder::new(entry.id, clean(&entry.title))
                    .content(entry.summary)
                    .author(entry.authors.join(", "))
                    .published_date(entry.published)
                    .metadata("doi", entry.doi)
                    .metadata("journal", entry.journal)
                    .metadata("comments", entry.comment)
                    .metadata("tags", entry.tags.join(", "))
                    .metadata("pdf_url", entry.pdf)
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[derive(Default)]
struct AtomEntry {
    title: String,
    id: String,
    summary: String,
    published: String,
    doi: String,
    journal: String,
    comment: String,
    pdf: String,
    authors: Vec<String>,
    tags: Vec<String>,
}

#[derive(Clone, Copy)]
enum Field {
    Title,
    Id,
    Summary,
    Published,
    AuthorName,
    Doi,
    Journal,
    Comment,
}

/// Read the Atom feed of the arXiv API. Only a structurally broken document is
/// an error; a valid feed without entries yields an empty list.
fn parse_atom(body: &str) -> Result<Vec<AtomEntry>, EngineError> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut entries = Vec::new();
    let mut entry: Option<AtomEntry> = None;
    let mut field: Option<Field> = None;
    let mut captured = String::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| EngineError::Parse(format!("invalid Atom response: {error}")))?;
        match event {
            Event::Start(start) => {
                match start.local_name().as_ref() {
                    b"entry" => {
                        entry = Some(AtomEntry::default());
                        field = None;
                        captured.clear();
                    }
                    b"title" if entry.is_some() => field = Some(Field::Title),
                    b"id" if entry.is_some() => field = Some(Field::Id),
                    b"summary" if entry.is_some() => field = Some(Field::Summary),
                    b"published" if entry.is_some() => field = Some(Field::Published),
                    b"name" if entry.is_some() => field = Some(Field::AuthorName),
                    b"doi" if entry.is_some() => field = Some(Field::Doi),
                    b"journal_ref" if entry.is_some() => field = Some(Field::Journal),
                    b"comment" if entry.is_some() => field = Some(Field::Comment),
                    b"link" | b"category" => {
                        if let Some(entry) = entry.as_mut() {
                            apply_attributes(entry, &start);
                        }
                    }
                    _ => {}
                }
                if field.is_some() {
                    captured.clear();
                }
            }
            Event::Empty(start) => {
                if let Some(entry) = entry.as_mut() {
                    if matches!(start.local_name().as_ref(), b"link" | b"category") {
                        apply_attributes(entry, &start);
                    }
                }
            }
            Event::Text(text) => {
                if field.is_some() {
                    if let Ok(value) = text.xml_content(quick_xml::XmlVersion::Explicit1_0) {
                        captured.push_str(&value);
                    }
                }
            }
            Event::End(end) => {
                if end.local_name().as_ref() == b"entry" {
                    if let Some(entry) = entry.take() {
                        entries.push(entry);
                    }
                } else if let (Some(entry), Some(field)) = (entry.as_mut(), field) {
                    let value = std::mem::take(&mut captured);
                    match field {
                        Field::Title => entry.title = value,
                        Field::Id => entry.id = value,
                        Field::Summary => entry.summary = value,
                        Field::Published => entry.published = value,
                        Field::AuthorName => entry.authors.push(value),
                        Field::Doi => entry.doi = value,
                        Field::Journal => entry.journal = value,
                        Field::Comment => entry.comment = value,
                    }
                }
                field = None;
                captured.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(entries)
}

/// Reads the arxiv:category/@term and atom:link[@title='pdf']/@href attributes.
fn apply_attributes(entry: &mut AtomEntry, start: &BytesStart<'_>) {
    let mut term: Option<String> = None;
    let mut is_pdf = false;
    let mut href: Option<String> = None;
    for attribute in start.attributes().flatten() {
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .map(|value| value.to_string())
            .unwrap_or_default();
        match attribute.key.local_name().as_ref() {
            b"term" => term = Some(value),
            b"title" => is_pdf = value == "pdf",
            b"href" => href = Some(value),
            _ => {}
        }
    }
    if let Some(term) = term {
        entry.tags.push(term);
    }
    if is_pdf {
        if let Some(href) = href {
            entry.pdf = href;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <title>ArXiv Query: search_query=all:rust</title>
  <entry>
    <id>http://arxiv.org/abs/2401.00001v1</id>
    <published>2024-01-01T00:00:00Z</published>
    <title>Attention Is All You Need</title>
    <summary>We propose a new network architecture.</summary>
    <author><name>Ashish Vaswani</name></author>
    <author><name>Noam Shazeer</name></author>
    <link href="http://arxiv.org/abs/2401.00001v1" rel="alternate" type="text/html"/>
    <link title="pdf" href="http://arxiv.org/pdf/2401.00001v1" rel="related" type="application/pdf"/>
    <arxiv:doi>10.1000/xyz</arxiv:doi>
    <arxiv:journal_ref>Journal of Examples</arxiv:journal_ref>
    <category term="cs.CL" scheme="http://arxiv.org/schemas/atom"/>
    <category term="cs.AI" scheme="http://arxiv.org/schemas/atom"/>
  </entry>
</feed>"#;

    fn engine() -> Arc<Arxiv> {
        Arxiv::from_config(&EngineConfig::new("arxiv", "arxiv", "arx", &["science"])).unwrap()
    }

    #[test]
    fn parses_atom_entries() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Attention Is All You Need");
                assert_eq!(result.url, "http://arxiv.org/abs/2401.00001v1");
                assert_eq!(
                    result.author.as_deref(),
                    Some("Ashish Vaswani, Noam Shazeer")
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
                    result.metadata.get("pdf_url").map(String::as_str),
                    Some("http://arxiv.org/pdf/2401.00001v1")
                );
                assert_eq!(
                    result.metadata.get("tags").map(String::as_str),
                    Some("cs.CL, cs.AI")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_query_url() {
        let query = Query::parse(
            "black holes",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with(BASE_URL));
        assert!(request.url.contains("start=20"));
        assert!(request.url.contains("max_results=10"));
        assert!(request.url.contains("black%20holes"));
    }
}
