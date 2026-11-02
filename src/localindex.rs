//! Local full-text index (BM25) built from our own crawl.
//!
//! This is the piece that makes RustXNG independent of upstream engines: a
//! query answered from here costs milliseconds instead of a fan-out.
//!
//! Two tokenisations are indexed for every page, because the crawler cannot
//! assume a language: the default tokenizer for space-separated text, and
//! character bigrams for Japanese/Chinese, which have no word delimiters.

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term, doc};

/// One page ready to be indexed.
pub struct Page<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub lang: &'a str,
}

/// One search hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub score: f32,
}

/// A name for the CJK tokenizer, registered once per index.
const BIGRAM: &str = "bigram";

/// Fields of the schema, kept together so the searcher can address them.
struct Fields {
    url: Field,
    title: Field,
    body: Field,
    title_ja: Field,
    body_ja: Field,
    lang: Field,
}

/// An opened local index.
pub struct LocalIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    /// Present only for the process that writes the index.
    writer: Option<std::sync::Mutex<IndexWriter>>,
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    // The plain tokenizer for space-separated languages.
    let plain = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    // Bigrams for languages without spaces.
    let ngram = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(BIGRAM)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    builder.add_text_field("url", STRING | STORED);
    builder.add_text_field("title", plain.clone() | STORED);
    builder.add_text_field("body", plain);
    builder.add_text_field("title_ja", ngram.clone() | STORED);
    builder.add_text_field("body_ja", ngram);
    builder.add_text_field("lang", STRING | STORED);
    builder.build()
}

/// True when the text contains Japanese or Chinese characters.
pub fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff))
}

impl LocalIndex {
    /// Open the index for reading. Safe to call from any number of processes,
    /// including while a crawl is writing.
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, false)
    }

    /// Open the index for writing. Only one process may hold this at a time.
    pub fn open_for_write(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, true)
    }

    fn open_inner(path: &Path, writable: bool) -> Result<Self, String> {
        // Only the writer creates the directory: a reader that finds nothing
        // must not litter the working tree with an empty index.
        if writable {
            std::fs::create_dir_all(path).map_err(|error| format!("create {path:?}: {error}"))?;
        } else if !path.exists() {
            return Err(format!("no index at {path:?}"));
        }
        let schema = build_schema();
        let index = match Index::open_in_dir(path) {
            Ok(index) => index,
            Err(_) => Index::create_in_dir(path, schema.clone())
                .map_err(|error| format!("create index: {error}"))?,
        };
        let tokenizer = TextAnalyzer::builder(
            NgramTokenizer::new(2, 2, false).map_err(|error| error.to_string())?,
        )
        .filter(LowerCaser)
        .build();
        index.tokenizers().register(BIGRAM, tokenizer);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|error| format!("reader: {error}"))?;
        let writer = if writable {
            Some(
                index
                    .writer(256_000_000)
                    .map_err(|error| format!("writer: {error}"))?,
            )
        } else {
            None
        };
        let schema = index.schema();
        let fields = Fields {
            url: schema.get_field("url").map_err(|error| error.to_string())?,
            title: schema
                .get_field("title")
                .map_err(|error| error.to_string())?,
            body: schema
                .get_field("body")
                .map_err(|error| error.to_string())?,
            title_ja: schema
                .get_field("title_ja")
                .map_err(|error| error.to_string())?,
            body_ja: schema
                .get_field("body_ja")
                .map_err(|error| error.to_string())?,
            lang: schema
                .get_field("lang")
                .map_err(|error| error.to_string())?,
        };
        Ok(Self {
            index,
            reader,
            fields,
            writer: writer.map(std::sync::Mutex::new),
        })
    }

    /// Number of indexed documents.
    pub fn document_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Whether this URL is already indexed (committed documents only).
    /// Lets a later crawl continue where the previous one stopped instead of
    /// indexing the same pages twice.
    pub fn has_url(&self, url: &str) -> bool {
        let searcher = self.reader.searcher();
        let query = TermQuery::new(
            Term::from_field_text(self.fields.url, url),
            IndexRecordOption::Basic,
        );
        searcher
            .search(&query, &TopDocs::with_limit(1).order_by_score())
            .map(|hits| !hits.is_empty())
            .unwrap_or(false)
    }

    /// Add one page. The caller decides when to commit.
    pub fn add(&self, page: &Page<'_>) -> Result<(), String> {
        let writer = self.writer.as_ref().ok_or("index opened read-only")?;
        let writer = writer.lock().map_err(|_| "writer poisoned")?;
        writer
            .add_document(doc!(
                self.fields.url => page.url.to_string(),
                self.fields.title => page.title.to_string(),
                self.fields.body => page.body.to_string(),
                self.fields.title_ja => page.title.to_string(),
                self.fields.body_ja => page.body.to_string(),
                self.fields.lang => page.lang.to_string(),
            ))
            .map_err(|error| format!("add document: {error}"))?;
        Ok(())
    }

    /// Flush everything added so far and make it searchable.
    pub fn commit(&self) -> Result<(), String> {
        if let Some(writer) = self.writer.as_ref() {
            let mut writer = writer.lock().map_err(|_| "writer poisoned")?;
            writer
                .commit()
                .map_err(|error| format!("commit: {error}"))?;
        }
        self.reader
            .reload()
            .map_err(|error| format!("reload: {error}"))?;
        Ok(())
    }

    /// Rank the indexed pages for a query.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, String> {
        let searcher = self.reader.searcher();
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        // Space-separated text goes through the standard parser.
        let parser = QueryParser::for_index(&self.index, vec![self.fields.title, self.fields.body]);
        if let Ok(parsed) = parser.parse_query(trimmed) {
            clauses.push((Occur::Should, parsed));
        }
        // CJK text has no spaces: match character bigrams instead.
        if has_cjk(trimmed) {
            let lowered: Vec<char> = trimmed.to_lowercase().chars().collect();
            for pair in lowered.windows(2) {
                if !has_cjk(&pair.iter().collect::<String>()) {
                    continue;
                }
                let gram: String = pair.iter().collect();
                for field in [self.fields.title_ja, self.fields.body_ja] {
                    clauses.push((
                        Occur::Should,
                        Box::new(TermQuery::new(
                            Term::from_field_text(field, &gram),
                            IndexRecordOption::WithFreqs,
                        )),
                    ));
                }
            }
        }
        if clauses.is_empty() {
            return Ok(Vec::new());
        }
        let query = BooleanQuery::new(clauses);
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|error| format!("search: {error}"))?;
        let mut hits = Vec::new();
        for (score, address) in top {
            let document: tantivy::TantivyDocument = searcher
                .doc(address)
                .map_err(|error| format!("doc: {error}"))?;
            let text = |field: Field| {
                document
                    .get_first(field)
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default()
            };
            hits.push(Hit {
                url: text(self.fields.url),
                title: text(self.fields.title),
                score,
            });
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_index() -> (TempDir, LocalIndex) {
        let dir = TempDir::new().unwrap();
        let index = LocalIndex::open_for_write(dir.path()).unwrap();
        (dir, index)
    }

    #[test]
    fn a_reader_does_not_create_the_directory() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("index-data");
        assert!(
            LocalIndex::open(&missing).is_err(),
            "a missing index is an error"
        );
        assert!(!missing.exists(), "the reader must not create it");
        // The writer creates it on demand.
        let index = LocalIndex::open_for_write(&missing).unwrap();
        index.commit().unwrap();
        assert!(missing.exists());
    }

    #[test]
    fn indexes_and_finds_english_pages() {
        let (_dir, index) = temp_index();
        index
            .add(&Page {
                url: "https://tokio.rs/tokio/tutorial",
                title: "Tutorial",
                body: "async runtime for rust with tasks and futures",
                lang: "en",
            })
            .unwrap();
        index
            .add(&Page {
                url: "https://example.com/other",
                title: "Other",
                body: "nothing to do with the query",
                lang: "en",
            })
            .unwrap();
        index.commit().unwrap();
        assert_eq!(index.document_count(), 2);
        let hits = index.search("async runtime rust", 5).unwrap();
        assert_eq!(hits[0].url, "https://tokio.rs/tokio/tutorial");
    }

    #[test]
    fn indexes_and_finds_japanese_pages() {
        let (_dir, index) = temp_index();
        index
            .add(&Page {
                url: "https://www.gsi.go.jp/",
                title: "国土地理院",
                body: "地図と測量の情報を提供しています",
                lang: "ja",
            })
            .unwrap();
        index.commit().unwrap();
        let hits = index.search("国土地理院 地図", 5).unwrap();
        assert_eq!(hits[0].url, "https://www.gsi.go.jp/");
    }
}
