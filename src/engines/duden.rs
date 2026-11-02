//! Duden engine, ported from searx/engines/duden.py.
//!
//! The German dictionary search page is parsed like upstream: every section
//! except the essay teasers is a result.
// engine-id: duden

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["dictionaries"],
    shortcut: "du",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.duden.de";

/// Duden, the German dictionary.
pub struct Duden {
    meta: EngineMeta,
}

impl Duden {
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

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Engine for Duden {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = base_url(&self.meta);
        let encoded = encode(&query.query_text());
        let url = if pageno <= 1 {
            format!("{base}/suchen/dudenonline/{encoded}")
        } else {
            format!(
                "{base}/suchen/dudenonline/{encoded}?search_api_fulltext=&page={}",
                pageno - 1
            )
        };
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // After the last page Duden redirects to a spelling correction, and a
        // missing page is simply empty like upstream.
        if response.status == 404 {
            return Ok(Vec::new());
        }
        if !(200..300).contains(&response.status) {
            return Err(EngineError::Message(format!(
                "duden returned HTTP status {}",
                response.status
            )));
        }
        let html = parse_html(&response.text());
        let base = base_url(&self.meta);
        let mut results = Vec::new();
        for section in select_elements(&html, "section")? {
            let classes = attr(section, "class").unwrap_or_default();
            if classes.contains("essay") {
                continue;
            }
            let Some(link) = child(section, "h2 a") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let content = child(section, "p").map(text).unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Duden> {
        Duden::from_config(&EngineConfig::new(
            "duden",
            "duden",
            "du",
            &["dictionaries"],
        ))
        .expect("duden engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_paged_search_urls() {
        let engine = engine();
        assert_eq!(
            engine.build_request(&query("Katzen"), 1).unwrap().url,
            "https://www.duden.de/suchen/dudenonline/Katzen"
        );
        assert_eq!(
            engine.build_request(&query("Katzen"), 4).unwrap().url,
            "https://www.duden.de/suchen/dudenonline/Katzen?search_api_fulltext=&page=3"
        );
    }

    #[test]
    fn parses_sections_but_skips_essays() {
        let html = r##"
        <main>
          <section class="vignette">
            <h2><a href="/rechtschreibung/Katze">Katze</a></h2>
            <p>Die Katze ist ein Haustier.</p>
          </section>
          <section class="essay-teaser">
            <h2><a href="/essay/katzen">Essay</a></h2>
            <p>Ein Essay.</p>
          </section>
          <section class="vignette">
            <p>Kein Link.</p>
          </section>
        </main>"##;
        let response = EngineResponse::new(200, "https://www.duden.de", html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("Katze")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Katze");
                assert_eq!(result.url, "https://www.duden.de/rechtschreibung/Katze");
                assert_eq!(result.content, "Die Katze ist ein Haustier.");
            }
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_page_is_empty() {
        let response = EngineResponse::new(404, "https://www.duden.de", Vec::new());
        assert!(
            engine()
                .parse(&response, &query("Katze"))
                .unwrap()
                .is_empty()
        );
    }
}
