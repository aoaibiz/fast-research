//! Tiger engine, ported from searx/engines/tiger.py.
//!
//! Upstream solves Tiger's arithmetic CAPTCHA on a separate page and caches the
//! resulting code. The frozen build_request interface cannot perform those
//! extra round trips, so the code is read from the tiger_code option.
// engine-id: tiger

use std::sync::Arc;

use scraper::Html;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "tig",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://tiger.ch";
const CATEGORIES: [&str; 2] = ["Websuche", "News"];

/// Tiger, a Swiss meta search engine.
pub struct Tiger {
    meta: EngineMeta,
}

impl Tiger {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let category = config.opt_str("tiger_category").unwrap_or("Websuche");
        if !CATEGORIES.contains(&category) {
            return Err("tiger: invalid tiger_category".to_string());
        }
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

    fn category(&self) -> &str {
        self.meta.opt_str("tiger_category").unwrap_or("Websuche")
    }
}

impl Engine for Tiger {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("w", query.query_text()),
            ("page", pageno.max(1).to_string()),
        ];
        let mut request = EngineRequest::get(format!(
            "{}/{}?{}",
            self.base_url(),
            self.category(),
            query_string(&pairs)
        ));
        // Setting Checked=1 shows related search terms, like upstream.
        if let Some(code) = self
            .meta
            .opt_str("tiger_code")
            .filter(|code| !code.is_empty())
        {
            request = request.cookie("Tiger.ch", &format!("Tiger.ch=&Code={code}&Checked=1"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        if self.category() == "News" {
            return Ok(news_results(&html, self.base_url()));
        }
        let Ok(rows) = select_elements(&html, "div#mainContainer table tr") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for row in rows {
            let Some(link) = child(row, "a[class*='weblink']") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() || !is_http_url(&href) {
                continue;
            }
            let content = child(row, "[class*='webbodynopic']")
                .map(text)
                .unwrap_or_default();
            results.push(ResultBuilder::new(href, title).content(content).build());
        }
        for suggestion in select_elements(&html, "a[class*='linkAnders']")? {
            let value = text(suggestion);
            if !value.is_empty() {
                results.push(EngineItem::Suggestion(value));
            }
        }
        Ok(results)
    }
}

fn news_results(html: &Html, base: &str) -> Vec<EngineItem> {
    let Ok(rows) = select_elements(html, "div#panNews > div") else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for row in rows {
        let Some(link) = child(row, "a[class*='webLink']") else {
            continue;
        };
        let Some(href) = attr(link, "href") else {
            continue;
        };
        let title = text(link);
        if title.is_empty() || !is_http_url(&href) {
            continue;
        }
        let mut builder = ResultBuilder::new(href, title);
        let date = child(row, "span[class*='help'] span")
            .map(text)
            .unwrap_or_default();
        let date = date.trim_start_matches('-').trim().to_string();
        if !date.is_empty() {
            builder = builder.published_date(date);
        }
        if let Some(src) = attr_of(row, "img", "src") {
            let thumbnail = if src.starts_with("http") {
                src
            } else {
                format!("{base}{src}")
            };
            builder = builder.thumbnail(thumbnail);
        }
        results.push(builder.build());
    }
    results
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEB_FIXTURE: &str = r##"<html><body><div id="mainContainer"><table><tr><td><a class="weblink" href="https://example.com/rust">Rust</a><div class="webbodynopic">A fast language.</div></td></tr><tr><td><a class="weblink" href="javascript:void(0)">Broken</a></td></tr></table></div><a class="linkAnders" href="/Websuche?w=rust+lang">rust lang</a></body></html>"##;

    const NEWS_FIXTURE: &str = r##"<html><body><div id="panNews"><div><img src="/img/tiger.png"><a class="webLink" href="https://example.com/news">Tiger news</a><span class="help"><span>- 2024-05-01</span></span></div></div></body></html>"##;

    fn engine(category: Option<&str>) -> Result<Arc<Tiger>, String> {
        let mut config = EngineConfig::new("tiger", "tiger", "tig", &["general"]);
        if let Some(value) = category {
            config
                .extra
                .insert("tiger_category".into(), toml::Value::String(value.into()));
        }
        Tiger::from_config(&config)
    }

    fn parse(category: Option<&str>, body: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        engine(category).unwrap().parse(&response, &query).unwrap()
    }

    #[test]
    fn parses_web_and_news_results() {
        let web = parse(None, WEB_FIXTURE);
        assert_eq!(web.len(), 2);
        let EngineItem::Result(result) = &web[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/rust");
        assert_eq!(result.content, "A fast language.");
        assert!(matches!(&web[1], EngineItem::Suggestion(value) if value == "rust lang"));
        let news = parse(Some("News"), NEWS_FIXTURE);
        let EngineItem::Result(result) = &news[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "Tiger news");
        assert_eq!(result.published_date.as_deref(), Some("2024-05-01"));
    }

    #[test]
    fn validates_configuration_and_builds_the_request() {
        assert!(engine(Some("Pictures")).is_err());
        let query = Query::parse("rust lang", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine(None).unwrap().build_request(&query, 2).unwrap();
        assert!(request.url.contains("w=rust%20lang"));
    }
}
