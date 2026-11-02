//! Ansa engine, ported from searx/engines/ansa.py.

// engine-id: ansa

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "ans",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.ansa.it";
const SEARCH_API: &str = "https://www.ansa.it/ricerca/ansait/search.shtml?";
const PAGE_SIZE: u32 = 12;

/// Ansa, the Italian news agency.
pub struct Ansa {
    meta: EngineMeta,
}

impl Ansa {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Ansa {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let page_size = self
            .meta
            .opt_i64("page_size")
            .filter(|value| *value > 0)
            .unwrap_or(i64::from(PAGE_SIZE)) as u32;
        let mut pairs = vec![
            ("any", query.query_text()),
            ("start", (pageno.saturating_sub(1) * page_size).to_string()),
            ("sort", "data:desc".to_string()),
        ];
        if let Some(days) = query.time_range.and_then(period_days) {
            pairs.push(("periodo", days.to_string()));
        }
        let endpoint = self.meta.search_url.as_deref().unwrap_or(SEARCH_API);
        Ok(EngineRequest::get(format!(
            "{endpoint}{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let mut results = Vec::new();
        for article in select_elements(&html, "div.article")? {
            let Some(link) = child(article, "div.content h2.title a") else {
                continue;
            };
            let (Some(href), title) = (attr(link, "href"), text(link)) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let content = text_of(article, "div.content div.text").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(src) = attr_of(article, "div.image a img", "src") {
                if let Some(thumbnail) = absolute_url(&src, base) {
                    builder = builder.thumbnail(thumbnail);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn period_days(range: TimeRange) -> Option<i64> {
    match range {
        TimeRange::Day => Some(1),
        TimeRange::Week => Some(7),
        TimeRange::Month => Some(31),
        TimeRange::Year => Some(365),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
<div class="article">
  <div class="image"><a href="/web/1"><img src="/media/one.jpg"></a></div>
  <div class="content">
    <h2 class="title"><a href="/web/2024/one.html">First news</a></h2>
    <div class="text">The text of the first news</div>
  </div>
</div>
<div class="article">
  <div class="content">
    <h2 class="title"><a href="https://www.ansa.it/web/2024/two.html">Second news</a></h2>
    <div class="text">The text of the second news</div>
  </div>
</div>
</body></html>"##;

    fn engine() -> Arc<Ansa> {
        Ansa::from_config(&EngineConfig::new("ansa", "ansa", "ans", &["news"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_article_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First news");
                assert_eq!(result.url, "https://www.ansa.it/web/2024/one.html");
                assert_eq!(result.content, "The text of the first news");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://www.ansa.it/media/one.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ansa.it/web/2024/two.html");
                assert!(result.thumbnail.is_none());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&query(), 3).unwrap();
        assert_eq!(
            request.url,
            "https://www.ansa.it/ricerca/ansait/search.shtml?any=rust&start=24&sort=data%3Adesc"
        );
    }

    #[test]
    fn appends_the_time_range_period() {
        let mut query = query();
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.ends_with("&periodo=7"));
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
