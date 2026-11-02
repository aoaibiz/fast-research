//! Ollama engine, ported from searx/engines/ollama.py.
//!
//! The model library search is scraped from the HTML page, like upstream.
// engine-id: ollama

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "repos"],
    shortcut: "ollama",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://ollama.com";
/// Upstream xpath: //li[@x-test-model].
const RESULTS: &str = "li[x-test-model]";
const TITLE: &str = "span[x-test-search-response-title]";
const CONTENT: &str = "p.max-w-lg.break-words.text-neutral-800.text-md";
const PUBLISHED: &str = "span[class*=\"flex items-center\"]";

pub struct Ollama {
    meta: EngineMeta,
}

impl Ollama {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Ollama {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("q", query.query_text())];
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
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, RESULTS)? {
            let Some(href) = attr_of(element, "a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let title = text_of(element, TITLE).unwrap_or_default();
            if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(text_of(element, CONTENT).unwrap_or_default());
            if let Some(published) =
                attr_of(element, PUBLISHED, "title").and_then(|value| date(&value))
            {
                builder = builder.published_date(published);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream parses "%b %d, %Y %I:%M %p %Z"; the zone suffix is optional here.
fn date(value: &str) -> Option<String> {
    let value = value.trim();
    let head = value
        .rsplit_once(' ')
        .map(|(head, _zone)| head)
        .unwrap_or(value);
    for candidate in [value, head] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(candidate, "%b %d, %Y %I:%M %p") {
            return Some(parsed.format("%Y-%m-%dT%H:%M:%S").to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Ollama> {
        Ollama::from_config(&EngineConfig::new("ollama", "ollama", "ollama", &["it"])).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_model_list() {
        let html = r##"<ul>
          <li x-test-model>
            <a href="/library/llama3.2">
              <span x-test-search-response-title>llama3.2</span>
              <p class="max-w-lg break-words text-neutral-800 text-md">Meta's Llama 3.2 model</p>
              <span class="flex items-center gap-2" title="Oct 24, 2026 11:00 AM UTC">new</span>
            </a>
          </li>
          <li x-test-model><a href="/library/broken"></a></li>
          <li><a href="/library/nope"><span x-test-search-response-title>nope</span></a></li>
        </ul>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("llama")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://ollama.com/library/llama3.2");
        assert_eq!(result.title, "llama3.2");
        assert_eq!(result.content, "Meta's Llama 3.2 model");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2026-10-24T11:00:00")
        );
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("llama 3.2"), 1).unwrap();
        assert_eq!(request.url, "https://ollama.com/search?q=llama%203.2");
        assert!(matches!(request.method, crate::engine::Method::Get));
        let empty = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&empty, &query("x")).unwrap().is_empty());
    }
}
