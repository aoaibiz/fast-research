//! Ahmia engine, ported from searx/engines/ahmia.py.

// engine-id: ahmia

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["onions"],
    shortcut: "",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    // Upstream sets 20 seconds: Tor is slow and a request may need two hops.
    timeout: 20.0,
};

const BASE_URL: &str = "http://juhanurmihxlp77nkq76byazcldy2hlmovfu2epvl5ankdibsot4csyd.onion";

/// Ahmia, the onion search engine.
pub struct Ahmia {
    meta: EngineMeta,
}

impl Ahmia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Ahmia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let mut pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        // Upstream scrapes the token from the search form; settings can provide it.
        if let Some(token) = self.meta.opt_str("token") {
            if let Some((name, value)) = token.split_once(':') {
                if !name.trim().is_empty() && !value.trim().is_empty() {
                    pairs.push((name.trim(), value.trim().to_string()));
                }
            }
        }
        let days = match query.time_range {
            Some(TimeRange::Day) => 1,
            Some(TimeRange::Week) => 7,
            Some(TimeRange::Month) => 30,
            _ => 0,
        };
        if days > 0 {
            pairs.push(("d", days.to_string()));
        }
        let encoded = query_string(&pairs);
        let url = match self.meta.search_url.as_deref() {
            Some(template) => template.replace("{query}", &encoded),
            None => format!("{base}/search/?{encoded}"),
        };
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let page_size = self
            .meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(10) as usize;
        let first = page_size * query.pageno.saturating_sub(1) as usize;
        let elements = select_elements(&html, "li.result")?;
        let mut results = Vec::new();
        for element in elements.into_iter().skip(first).take(page_size) {
            let Some(link) = child(element, "h4 a") else {
                continue;
            };
            let (Some(href), title) = (attr(link, "href"), text(link)) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            // Upstream drops the Ahmia wrapper and keeps the redirect_url link.
            let Some(url) = absolute_url(&href, base).map(|url| unwrap_url_parameter(&url)) else {
                continue;
            };
            if url.is_empty() || !is_http_url(&url) {
                continue;
            }
            let content = text_of(element, "p").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        // Upstream returns the spelling suggestion as a correction.
        if let Ok(Some(correction)) = select_one(&html, "#didYouMean a") {
            let suggestion = text(correction);
            if !suggestion.is_empty() {
                results.push(EngineItem::Correction(suggestion));
            }
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

    const FIXTURE: &str = r##"<html><body>
<ul>
  <li class="result">
    <h4><a href="/search/?q=rust&amp;redirect_url=http%3A%2F%2Fexampleonion.onion%2Fpage">Onion page</a></h4>
    <p>An onion snippet</p>
  </li>
  <li class="result">
    <h4><a href="/search/?q=rust&amp;redirect_url=http%3A%2F%2Fexampleonion.onion%2Fsecond">Second page</a></h4>
    <p>Another snippet</p>
  </li>
</ul>
<div id="didYouMean"><a href="/search/?q=rust+language">rust language</a></div>
</body></html>"##;

    fn engine_with(extra: &[(&str, &str)]) -> Arc<Ahmia> {
        let mut config = EngineConfig::new("ahmia", "ahmia", "", &["onions"]);
        for (key, value) in extra {
            let number = value.parse::<i64>().ok().map(toml::Value::Integer);
            let parsed = number.unwrap_or(toml::Value::String((*value).to_string()));
            config.extra.insert((*key).to_string(), parsed);
        }
        Ahmia::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn unwraps_the_redirect_urls() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine_with(&[]).parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 3);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Onion page");
                assert_eq!(result.url, "http://exampleonion.onion/page");
                assert_eq!(result.content, "An onion snippet");
            }
            _ => panic!("expected a result"),
        }
        assert!(matches!(&items[2], EngineItem::Correction(text) if text == "rust language"));
    }

    #[test]
    fn pages_the_trimmed_result_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let mut query = query();
        query.pageno = 2;
        let items = engine_with(&[("page_size", "1")])
            .parse(&response, &query)
            .unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "http://exampleonion.onion/second")
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_and_time_range_request() {
        let mut query = query();
        query.time_range = Some(TimeRange::Week);
        let engine = engine_with(&[("token", "csrf:abc123")]);
        let request = engine.build_request(&query, 1).unwrap();
        let expected = "http://juhanurmihxlp77nkq76byazcldy2hlmovfu2epvl5ankdibsot4csyd.onion/search/?q=rust&csrf=abc123&d=7";
        assert_eq!(request.url, expected);
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        let items = engine_with(&[]).parse(&response, &query()).unwrap();
        assert!(items.is_empty());
    }
}
