//! Seznam engine, ported from searx/engines/seznam.py.
//!
//! Upstream fetches the search page first to collect its hidden fields and
//! cookies; the frozen build_request interface cannot do that extra round trip,
//! so only q and oq are sent and the verify redirect is reported as an access
//! denial.
// engine-id: seznam

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "szn",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://search.seznam.cz/";

/// Seznam web search.
pub struct Seznam {
    meta: EngineMeta,
}

impl Seznam {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Seznam {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        let pairs = [("q", query.query_text()), ("oq", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{base}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if response.final_url.contains("/verify") {
            return Err(EngineError::AccessDenied);
        }
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "#searchpage-root .Layout--left .f2c528")? {
            let Some(title_element) = child(element, "h3 a") else {
                continue;
            };
            let Some(data_element) = child(element, "div.c8774a, div.e69e8d.a11657") else {
                continue;
            };
            let Some(href) = attr(title_element, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, BASE_URL) else {
                continue;
            };
            let title = text(title_element);
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(text(data_element))
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<Seznam> {
        Seznam::from_config(&EngineConfig::new("seznam", "seznam", "szn", &["general"]))
            .expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_results_page() {
        let html = r##"<html><body><div id="searchpage-root"><div class="Layout--left">
          <div class="f2c528"><h3><a href="https://www.rust-lang.org/">Rust</a></h3>
            <div class="c8774a">A language empowering everyone.</div></div>
          <div class="f2c528"><h3><a href="https://doc.rust-lang.org/">Docs</a></h3>
            <div class="e69e8d a11657">Documentation.</div></div>
        </div></div></body></html>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language empowering everyone.");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.content, "Documentation."),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn verify_redirects_are_access_denied() {
        let response = EngineResponse::new(
            200,
            "https://search.seznam.cz/verify?url=x",
            b"<html></html>".to_vec(),
        );
        assert!(matches!(
            engine().parse(&response, &query("rust")),
            Err(EngineError::AccessDenied)
        ));
        let empty = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&empty, &query("rust")).unwrap().is_empty());
    }

    #[test]
    fn request_sends_both_query_parameters() {
        let request = engine().build_request(&query("rust"), 1).unwrap();
        assert_eq!(request.url, "https://search.seznam.cz/?q=rust&oq=rust");
    }
}
