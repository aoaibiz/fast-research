//! Rumble engine, ported from searx/engines/rumble.py.
// engine-id: rumble

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "ru",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://rumble.com/";

/// The HTML search of Rumble.
pub struct Rumble {
    meta: EngineMeta,
}

impl Rumble {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        if base.ends_with('/') {
            base
        } else {
            format!("{base}/")
        }
    }
}

impl Engine for Rumble {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        if pageno > 1 {
            pairs.push(("page", pageno.to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{}search/video?{}",
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
        for element in select_elements(&html, "li[class*=\"video-listing-entry\"]")? {
            let Some(href) = attr_of(element, "a.video-item--a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let title = text_of(element, "h3.video-item--title").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let views =
                attr_of(element, "span.video-item--views", "data-value").unwrap_or_default();
            let rumbles =
                attr_of(element, "span.video-item--rumbles", "data-value").unwrap_or_default();
            let earned =
                attr_of(element, "span.video-item--earned", "data-value").unwrap_or_default();
            let mut content = format!("{views} views - {rumbles} rumbles");
            if !earned.is_empty() {
                content.push_str(&format!(" - ${earned}"));
            }
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(content)
                .author(text_of(element, "div.ellipsis-1").unwrap_or_default())
                .published_date(fixed_date(
                    &attr_of(element, "time.video-item--time", "datetime").unwrap_or_default(),
                ))
                .metadata(
                    "length",
                    attr_of(element, "span.video-item--duration", "data-value").unwrap_or_default(),
                );
            let thumbnail = attr_of(element, "img.video-item--img", "src").unwrap_or_default();
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream calls datetime.fromisoformat and keeps the offset of the value.
fn fixed_date(value: &str) -> String {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return date.to_rfc3339();
    }
    if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return date.format("%Y-%m-%dT%H:%M:%S").to_string();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body><ul class="videostream">
      <li class="video-listing-entry video-listing-entry--grid">
        <a class="video-item--a" href="/v12345-rust-talk.html">
          <img class="video-item--img" src="https://sp.rmbl.ws/thumb.jpg">
          <h3 class="video-item--title">Rust talk</h3>
        </a>
        <time class="video-item--meta video-item--time" datetime="2024-05-01T12:00:00-04:00">2 days ago</time>
        <span class="video-item--meta video-item--views" data-value="1234">1.2K</span>
        <span class="video-item--meta video-item--rumbles" data-value="56">56</span>
        <span class="video-item--meta video-item--earned" data-value="12.34">12</span>
        <span class="video-item--duration" data-value="312">5:12</span>
        <div class="ellipsis-1">Some Channel</div>
      </li>
    </ul></body></html>"##;

    fn engine() -> Arc<Rumble> {
        Rumble::from_config(&EngineConfig::new("rumble", "rumble", "ru", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_listing_entries() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust talk");
                assert_eq!(result.url, "https://rumble.com/v12345-rust-talk.html");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.author.as_deref(), Some("Some Channel"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://sp.rmbl.ws/thumb.jpg")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:00:00-04:00")
                );
                assert_eq!(result.content, "1234 views - 56 rumbles - $12.34");
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("312")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_missing_listing_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }

    #[test]
    fn paging_is_forwarded_after_the_first_page() {
        assert_eq!(
            engine().build_request(&query(), 1).unwrap().url,
            "https://rumble.com/search/video?q=rust"
        );
        assert_eq!(
            engine().build_request(&query(), 2).unwrap().url,
            "https://rumble.com/search/video?q=rust&page=2"
        );
    }
}
