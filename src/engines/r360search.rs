//! 360search engine, ported from `searx/engines/360search.py`.
// engine-id: 360search

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "360",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.so.com";

const TIME_RANGE: [(TimeRange, &str); 4] = [
    (TimeRange::Day, "d"),
    (TimeRange::Week, "w"),
    (TimeRange::Month, "m"),
    (TimeRange::Year, "y"),
];

/// 360search web search engine.
pub struct Search360 {
    meta: EngineMeta,
}

impl Search360 {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Search360 {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut pairs = vec![("pn", pageno.to_string()), ("q", query.query_text())];
        if let Some(range) = query.time_range {
            if let Some(value) = TIME_RANGE.iter().find(|(r, _)| *r == range) {
                pairs.push(("adv_t", value.1.to_string()));
            }
        }
        Ok(EngineRequest::get(format!(
            "{base}/s?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.trim().is_empty() {
            return Ok(Vec::new());
        }
        let html = parse_html(&body);
        let mut results = Vec::new();
        for item in select_elements(&html, "li.res-list")? {
            let Some(link) = child(item, "h3.res-title a") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let url = attr(link, "data-mdurl")
                .or_else(|| attr(link, "href"))
                .unwrap_or_default();
            if url.is_empty() || !is_http_url(&url) {
                continue;
            }
            let content = text_of(item, "p.res-desc")
                .or_else(|| text_of(item, "span.res-list-summary"))
                .unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
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

    #[test]
    fn parses_the_result_list() {
        let html = r#"
        <div>
          <li class="res-list">
            <h3 class="res-title"><a href="https://example.com/a" data-mdurl="https://example.com/a">Alpha</a></h3>
            <p class="res-desc">Alpha description</p>
          </li>
          <li class="res-list">
            <h3 class="res-title"><a href="https://example.com/b">Beta</a></h3>
            <span class="res-list-summary">Beta summary</span>
          </li>
        </div>"#;
        let response = EngineResponse::new(200, "https://www.so.com/", html.as_bytes().to_vec());
        let engine = Search360::from_config(&EngineConfig::new(
            "360search",
            "360search",
            "360",
            &["general"],
        ))
        .unwrap();
        let query = Query::parse("alpha", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/a");
                assert_eq!(result.title, "Alpha");
                assert_eq!(result.content, "Alpha description");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_url_with_time_range() {
        let engine = Search360::from_config(&EngineConfig::new(
            "360search",
            "360search",
            "360",
            &["general"],
        ))
        .unwrap();
        let mut query = Query::parse("alpha", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(request.url.contains("adv_t=w"));
        assert!(request.url.contains("pn=2"));
        assert!(request.url.contains("q=alpha"));
    }
}
