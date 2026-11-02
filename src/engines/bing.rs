//! Bing engine, ported from `searx/engines/bing.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "bi",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.bing.com/search";

/// Bing web search.
pub struct Bing {
    meta: EngineMeta,
}

impl Bing {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Bing {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let region = locale_region(&query.locale);
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            (
                "adlt",
                match query.safesearch {
                    0 => "off",
                    1 => "moderate",
                    _ => "strict",
                }
                .to_string(),
            ),
            ("first", ((pageno - 1) * 10 + 1).to_string()),
            ("count", "10".to_string()),
        ];
        if language != "all" {
            pairs.push(("setlang", language.clone()));
        }
        if region != "all" && !matches!(region.as_str(), "us" | "cn" | "ru") {
            pairs.push(("cc", region));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "ez1",
                TimeRange::Week => "ez2",
                TimeRange::Month => "ez3",
                TimeRange::Year => "ez5",
            };
            pairs.push(("filters", format!("ex1%3a%22{value}%22")));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept-Language", &format!("{language};q=0.9")),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "li.b_algo")? {
            let Some(link) = child(element, "h2 a") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://www.bing.com/") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let content = text_of(element, "div.b_caption p")
                .or_else(|| text_of(element, "p"))
                .or_else(|| text_of(element, "div.b_snippet"))
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(date) = text_of(element, "span.news_dt") {
                builder = builder.published_date(date);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}
