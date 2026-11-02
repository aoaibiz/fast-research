//! Yandex engine, ported from searx/engines/yandex.py.
//!
//! The web search type is ported; the images search type needs the JSON island
//! of the result page and is documented as not ported.
// engine-id: yandex

use std::sync::Arc;

use chrono::Datelike;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "yd",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://yandex.com/search/site/";

/// Languages Yandex accepts in the lang parameter.
const LANGS: &[&str] = &["ru", "en", "be", "fr", "de", "id", "kk", "tt", "tr", "uk"];

/// Yandex web search.
pub struct Yandex {
    meta: EngineMeta,
}

impl Yandex {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let search_type = config.opt_str("search_type").unwrap_or("web");
        if search_type != "web" {
            return Err(format!("yandex: unsupported search_type {search_type:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Yandex {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        let mut pairs: Vec<(&str, String)> = vec![("text", query.query_text())];
        if pageno > 1 {
            pairs.push(("p", (pageno - 1).to_string()));
        }
        let language = locale_language(&query.locale);
        if LANGS.contains(&language.as_str()) {
            pairs.push(("lang", language));
        }
        if let Some(range) = query.time_range {
            let days = match range {
                TimeRange::Day => 1,
                TimeRange::Week => 7,
                TimeRange::Month => 30,
                TimeRange::Year => 365,
            };
            let start = chrono::Utc::now() - chrono::Duration::days(days);
            let end = chrono::Utc::now();
            pairs.push(("within", "777".to_string()));
            pairs.push(("from_day", start.day().to_string()));
            pairs.push(("from_month", start.month().to_string()));
            pairs.push(("from_year", start.year().to_string()));
            pairs.push(("to_day", end.day().to_string()));
            pairs.push(("to_month", end.month().to_string()));
            pairs.push(("to_year", end.year().to_string()));
        }
        pairs.push(("tmpl_version", "releases".to_string()));
        pairs.push(("web", "1".to_string()));
        pairs.push(("frame", "1".to_string()));
        pairs.push(("searchid", "3131712".to_string()));
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
        if response.header("x-yandex-captcha") == Some("captcha") {
            return Err(EngineError::Captcha);
        }
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "li[class*='serp-item']")? {
            let title = text_of(element, "h3.b-serp-item__title a.b-serp-item__title-link")
                .or_else(|| text_of(element, "h3.b-serp-item__title"))
                .unwrap_or_default();
            let Some(href) = attr_of(element, "a.b-serp-item__title-link", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, "https://yandex.com/") else {
                continue;
            };
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let content = text_of(element, "div.b-serp-item__content div.b-serp-item__text")
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(stamp) = attr_of(element, "span[data-mtime]", "data-mtime") {
                if let Ok(seconds) = stamp.parse::<i64>() {
                    if let Some(date) = chrono::DateTime::from_timestamp(seconds, 0) {
                        builder =
                            builder.published_date(date.format("%Y-%m-%dT%H:%M:%SZ").to_string());
                    }
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<Yandex> {
        Yandex::from_config(&EngineConfig::new("yandex", "yandex", "yd", &["general"]))
            .expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_web_results_page() {
        let html = r##"<html><body><ul>
          <li class="serp-item">
            <h3 class="b-serp-item__title"><a class="b-serp-item__title-link" href="https://www.rust-lang.org/">Rust</a></h3>
            <div class="b-serp-item__content"><div class="b-serp-item__text">A language empowering everyone.</div></div>
            <span data-mtime="1700000000"></span>
          </li>
        </ul></body></html>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language empowering everyone.");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2023-11-14T22:13:20Z")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn time_range_and_language_are_forwarded() {
        let mut request_query = query("rust");
        request_query.locale = "ru-RU".to_string();
        request_query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&request_query, 2).unwrap();
        assert!(request.url.starts_with("https://yandex.com/search/site/?"));
        assert!(request.url.contains("text=rust"));
        assert!(request.url.contains("p=1"));
        assert!(request.url.contains("lang=ru"));
        assert!(request.url.contains("within=777"));
        assert!(request.url.contains("from_day="));
        assert!(request.url.contains("searchid=3131712"));
    }

    #[test]
    fn captcha_and_empty_pages_are_handled() {
        let query = query("rust");
        let mut response = EngineResponse::new(200, BASE_URL, Vec::new());
        response
            .headers
            .insert("x-yandex-captcha".to_string(), "captcha".to_string());
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Captcha)
        ));
        let empty = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        assert!(engine().parse(&empty, &query).unwrap().is_empty());
    }
}
