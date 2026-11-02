//! Wordnik engine, ported from searx/engines/wordnik.py.
//!
//! Upstream emits one translation item per dictionary source; this port merges
//! the definitions into one result because the result container keys on the URL.
// engine-id: wordnik

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["dictionaries", "define"],
    shortcut: "wnik",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

const BASE_URL: &str = "https://www.wordnik.com";

/// Wordnik, the English dictionary.
pub struct Wordnik {
    meta: EngineMeta,
}

impl Wordnik {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn word_url(meta: &EngineMeta, word: &str) -> String {
    format!("{}/words/{}", base_url(meta), encode(word.trim()))
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Engine for Wordnik {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(word_url(
            &self.meta,
            &query.query_text(),
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let Some(define) = select_one(&html, "#define")? else {
            return Ok(Vec::new());
        };
        let title = clean(&query.query_text());
        if title.is_empty() {
            return Ok(Vec::new());
        }
        let url = word_url(&self.meta, &title);
        if !is_http(&url) {
            return Ok(Vec::new());
        }
        let mut definitions: Vec<String> = Vec::new();
        for list in children(define, "ul") {
            for item in children(list, "li") {
                let full = text(item);
                if full.is_empty() {
                    continue;
                }
                let abbreviation = text_of(item, "abbr").unwrap_or_default();
                let definition = if abbreviation.is_empty() {
                    full
                } else {
                    full.strip_prefix(&abbreviation)
                        .unwrap_or(&full)
                        .trim()
                        .to_string()
                };
                if definition.is_empty() {
                    continue;
                }
                if !definitions.iter().any(|existing| existing == &definition) {
                    definitions.push(definition);
                }
            }
        }
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![
            ResultBuilder::new(url, title)
                .content(definitions.join(" "))
                .build(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Wordnik> {
        Wordnik::from_config(&EngineConfig::new(
            "wordnik",
            "wordnik",
            "wnik",
            &["dictionaries", "define"],
        ))
        .expect("wordnik engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_word_url() {
        assert_eq!(
            engine().build_request(&query("cat"), 1).unwrap().url,
            "https://www.wordnik.com/words/cat"
        );
    }

    #[test]
    fn merges_the_definitions_of_every_source() {
        let html = r##"
        <div id="define">
          <h3 class="source">American Heritage Dictionary</h3>
          <ul>
            <li><abbr>n.</abbr> A domesticated carnivorous mammal.</li>
            <li><abbr>v.</abbr> To pursue.</li>
          </ul>
          <h3 class="source">WordNet</h3>
          <ul>
            <li>A small domesticated mammal.</li>
          </ul>
        </div>"##;
        let response =
            EngineResponse::new(200, "https://www.wordnik.com", html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("cat")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "cat");
                assert_eq!(result.url, "https://www.wordnik.com/words/cat");
                assert_eq!(
                    result.content,
                    "A domesticated carnivorous mammal. To pursue. A small domesticated mammal."
                );
            }
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn a_page_without_definitions_is_empty() {
        let html = r##"<html><body><p>No entry.</p></body></html>"##;
        let response =
            EngineResponse::new(200, "https://www.wordnik.com", html.as_bytes().to_vec());
        assert!(engine().parse(&response, &query("cat")).unwrap().is_empty());
    }
}
