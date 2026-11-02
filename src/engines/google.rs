//! Google engine, ported from `searx/engines/google.py`.

use std::collections::HashSet;
use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "go",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.google.com/search";

/// Google web search (HTML endpoint).
pub struct Google {
    meta: EngineMeta,
}

impl Google {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Google {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("num", "10".to_string()),
            ("start", ((pageno - 1) * 10).to_string()),
            // the basic HTML endpoint is much easier to parse than the JS one
            ("gbv", "1".to_string()),
            ("filter", "0".to_string()),
        ];
        if language != "all" {
            pairs.push(("hl", language.clone()));
            pairs.push(("lr", format!("lang_{language}")));
        }
        let safe = if query.safesearch >= 1 {
            "active"
        } else {
            "off"
        };
        pairs.push(("safe", safe.to_string()));
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "qdr:d",
                TimeRange::Week => "qdr:w",
                TimeRange::Month => "qdr:m",
                TimeRange::Year => "qdr:y",
            };
            pairs.push(("tbs", value.to_string()));
        }
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url)
            .header("Accept-Language", &format!("{language},*;q=0.5"))
            .header("Sec-Fetch-Site", "same-origin"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        if body.contains("detected unusual traffic") || body.contains("/sorry/index") {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&body);
        let blocked = !body.contains("did not match any documents")
            && !body.contains("/url?q=")
            && !body.contains("MjjYud")
            && !body.contains("class=\"g\"");
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for element in select_elements(&html, "div.g, div.tF2Cxc, div.MjjYud")? {
            let Some(link) = child(element, "a[href]") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = unwrap_google_url(&href) else {
                continue;
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            let title = text_of(element, "h3").unwrap_or_else(|| text(link));
            if title.is_empty() || title.starts_with("http") {
                continue;
            }
            let content = text_of(element, "div.VwiC3b")
                .or_else(|| text_of(element, "span.aCOpRe"))
                .or_else(|| text_of(element, "div[data-sncf]"))
                .or_else(|| text_of(element, "div.IsZvec"))
                .unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        if results.is_empty() && blocked {
            // Google serves an interstitial (no result markup at all) to
            // clients it does not trust; report it instead of "no results".
            return Err(EngineError::AccessDenied);
        }
        Ok(results)
    }
}

/// Unwrap the `/url?q=...` redirects Google uses in its result links.
fn unwrap_google_url(href: &str) -> Option<String> {
    if let Some(rest) = href.strip_prefix("/url?") {
        for pair in rest.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or_default();
            let value = parts.next().unwrap_or_default();
            if key == "q" || key == "url" {
                if let Ok(decoded) = percent_encoding::percent_decode_str(value).decode_utf8() {
                    return Some(decoded.to_string());
                }
            }
        }
    }
    if href.starts_with('/') {
        return Some(format!("https://www.google.com{href}"));
    }
    absolute_url(href, "https://www.google.com/")
}
