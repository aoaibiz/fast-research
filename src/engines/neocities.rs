//! Neocities engine, ported from searx/engines/neocities.py.
//!
//! Neocities is a static site hosting service; the HTML result page is parsed
//! with the same selectors as upstream.
// engine-id: neocities

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "blogs"],
    shortcut: "nc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://neocities.org";
const PAGE_SIZE: u32 = 100;

/// Neocities site search.
pub struct Neocities {
    meta: EngineMeta,
}

impl Neocities {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for Neocities {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        if pageno > 1 {
            let offset = (pageno - 1) * PAGE_SIZE;
            pairs.push(("start", offset.to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.base_url().to_string();
        let mut results = Vec::new();
        for element in select_elements(&html, "div.result-item")? {
            let Some(href) = attr_of(element, "div.result-url a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let Some(title) = text_of(element, "h3.result-title a") else {
                continue;
            };
            let content = text_of(element, "p.result-snippet").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(screenshot) = attr_of(element, "a.result-screenshot img", "src") {
                if let Some(thumbnail) = absolute_url(&screenshot, &base) {
                    if is_http_url(&thumbnail) {
                        builder = builder.thumbnail(thumbnail);
                    }
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <div class="result-item">
      <div class="result-url"><a href="/site/example">example.neocities.org</a></div>
      <h3 class="result-title"><a href="/site/example">Example site</a></h3>
      <p class="result-snippet">A personal site about <b>rust</b>.</p>
      <a class="result-screenshot" href="/site/example"><img src="/screenshots/example.png"></a>
    </div>
    <div class="result-item">
      <div class="result-url"><a href="/site/empty">empty.neocities.org</a></div>
      <h3 class="result-title"><a></a></h3>
    </div>"#;

    fn engine() -> Arc<Neocities> {
        Neocities::from_config(&EngineConfig::new(
            "neocities",
            "neocities",
            "nc",
            &["general"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_results_and_resolves_relative_urls() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://neocities.org/site/example");
                assert_eq!(result.title, "Example site");
                assert_eq!(result.content, "A personal site about rust .");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://neocities.org/screenshots/example.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn paginates_in_steps_of_one_hundred() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let first = engine().build_request(&query, 1).unwrap();
        assert!(first.url.contains("q=rust"));
        assert!(!first.url.contains("start="));
        let third = engine().build_request(&query, 3).unwrap();
        assert!(third.url.contains("start=200"));
    }
}
