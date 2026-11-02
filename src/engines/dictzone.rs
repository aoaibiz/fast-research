//! Dictzone engine, ported from searx/engines/dictzone.py.
//!
//! Dictzone is an offline-capable online dictionary; upstream scrapes the
//! two-column translation table.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: dictzone

const BASE_URL: &str = "https://dictzone.com";

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "translate"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// Dictzone translation search.
pub struct Dictzone {
    meta: EngineMeta,
}

impl Dictzone {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

/// Upstream maps the locale to a dictionary name such as "english-german".
///
/// "en-de" means English to German; a plain locale translates to itself.
fn dictionary_name(locale: &str) -> (String, String) {
    let mut parts = locale.split(['-', '_']);
    let from = parts.next().unwrap_or("en").to_lowercase();
    let to = parts
        .next()
        .map(str::to_lowercase)
        .unwrap_or_else(|| from.clone());
    (language_name(&from), language_name(&to))
}

fn language_name(language: &str) -> String {
    let name = match language {
        "en" => "english",
        "de" => "german",
        "fr" => "french",
        "es" => "spanish",
        "it" => "italian",
        "pl" => "polish",
        "ru" => "russian",
        "hu" => "hungarian",
        "cs" => "czech",
        "sk" => "slovak",
        "pt" => "portuguese",
        "nl" => "dutch",
        "tr" => "turkish",
        "ja" => "japanese",
        "zh" => "chinese",
        other => other,
    };
    name.to_string()
}

impl Engine for Dictzone {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let (from, to) = dictionary_name(&query.locale);
        Ok(EngineRequest::get(format!(
            "{base}/{from}-{to}-dictionary/{}",
            encode(&query.query_text())
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let url = format!("{base}/{}", encode(&query.query_text()));
        let mut results = Vec::new();
        for row in select_elements(&html, "table#r tr")? {
            let cells = children(row, "td");
            if cells.len() != 2 {
                continue;
            }
            let source = text(cells[0]);
            if source.is_empty() {
                continue;
            }
            let mut content = source.clone();
            let mut first = true;
            for paragraph in children(cells[1], "p") {
                let mut line = text(paragraph);
                if let Some(sample) = child(paragraph, "i.smpl") {
                    let sample = text(sample);
                    if !sample.is_empty() {
                        line = format!("{line} // {sample}");
                    }
                }
                if first {
                    content = format!("{source} : {line}");
                    first = false;
                }
            }
            results.push(
                ResultBuilder::new(url.clone(), source)
                    .content(content)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Dictzone> {
        Dictzone::from_config(&EngineConfig::new("dictzone", "dictzone", "", &["general"]))
            .expect("engine")
    }

    #[test]
    fn builds_the_dictionary_url() {
        let mut query = Query::parse("hello", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "en-DE".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://dictzone.com/english-german-dictionary/hello")
        );
    }

    #[test]
    fn parses_the_translation_rows() {
        let html = r##"<table id="r">
            <tr><th>English</th><th>German</th></tr>
            <tr><td><i>hello</i> world</td><td><p>hallo</p><p>servus</p></td></tr>
        </table>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let query = Query::parse("hello", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "hello world");
                assert!(result.content.contains("hallo"));
            }
            _ => panic!("expected a result"),
        }
    }
}
