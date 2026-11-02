//! Mojeek engine, ported from `searx/engines/mojeek.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "mjk",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.mojeek.com/search";

/// Mojeek, an independent crawler based search engine.
pub struct Mojeek {
    meta: EngineMeta,
}

impl Mojeek {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Mojeek {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("safe", query.safesearch.min(1).to_string()),
        ];
        if pageno > 1 {
            pairs.push(("s", (10 * (pageno - 1)).to_string()));
        }
        if let Some(range) = query.time_range {
            let now = chrono::Utc::now();
            let since = match range {
                TimeRange::Day => now - chrono::Duration::days(1),
                TimeRange::Week => now - chrono::Duration::weeks(1),
                TimeRange::Month => now - chrono::Duration::days(31),
                TimeRange::Year => now - chrono::Duration::days(365),
            };
            pairs.push(("since", since.format("%Y%m%d").to_string()));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .cookie("language", &locale_language(&query.locale))
                .cookie("region", &locale_region(&query.locale)),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        // Mojeek answers with an Altcha proof-of-work page when it rate limits.
        if body.contains("altcha") || body.contains("captcha") {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let mut results = Vec::new();
        for link in select_elements(&html, "ul.results-standard li a.ob, ul.results li a.title")? {
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://www.mojeek.com/") else {
                continue;
            };
            let Some(item) = link.parent().and_then(scraper::ElementRef::wrap) else {
                continue;
            };
            let title = text_of(item, "h2 a").unwrap_or_else(|| text(link));
            if title.is_empty() {
                continue;
            }
            let content = text_of(item, "p.s").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}
