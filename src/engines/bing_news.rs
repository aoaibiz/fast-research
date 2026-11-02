//! Bing News engine, ported from `searx/engines/bing_news.py`.
//!
//! Deviation (documented in docs/PORTING.md): upstream uses the internal
//! `/news/infinitescrollajax` JSON endpoint, this port uses the RSS feed of the
//! same service; the `qft` time windows are the upstream ones.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "bnews",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.bing.com/news/search";

/// Bing News.
pub struct BingNews {
    meta: EngineMeta,
}

impl BingNews {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for BingNews {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let region = locale_region(&query.locale);
        let mut pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("format", "rss".to_string())];
        // Upstream's time_map: day=4, week=7, month=9 (Bing has no year window).
        if let Some(range) = query.time_range {
            let interval = match range {
                TimeRange::Day => Some("4"),
                TimeRange::Week => Some("7"),
                TimeRange::Month => Some("9"),
                TimeRange::Year => None,
            };
            if let Some(interval) = interval {
                pairs.push(("qft", format!("interval=\"{interval}\"")));
            }
        }
        if language != "all" {
            pairs.push(("setlang", language));
        }
        if region != "all" {
            pairs.push(("cc", region));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/rss+xml, application/xml"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let items = parse_rss(&response.text())?;
        let mut results = Vec::new();
        for item in items {
            if item.title.is_empty() || item.link.is_empty() {
                continue;
            }
            // Bing wraps the article URL in an apiclick redirect.
            let url = unwrap_url_parameter(&item.link);
            let mut content = strip_tags(&item.description);
            if let Some(source) = item.source.as_deref() {
                if !source.is_empty() {
                    content = if content.is_empty() {
                        source.to_string()
                    } else {
                        format!("{source} — {content}")
                    };
                }
            }
            results.push(
                ResultBuilder::new(url, item.title)
                    .content(content)
                    .published_date(item.pub_date.unwrap_or_default())
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="utf-8" ?>
<rss version="2.0"><channel>
  <item>
    <title>Rust in the news</title>
    <link>http://www.bing.com/news/apiclick.aspx?ref=FexRss&amp;aid=&amp;tid=1&amp;url=https%3a%2f%2fexample.com%2frust-news</link>
    <description>Something happened.</description>
    <pubDate>Sat, 17 Oct 2026 03:00:00 GMT</pubDate>
  </item>
</channel></rss>"#;

    fn engine() -> Arc<BingNews> {
        BingNews::from_config(&EngineConfig::new(
            "bing news",
            "bing_news",
            "bnews",
            &["news"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_rss_feed_and_unwraps_the_redirect() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/rust-news");
                assert_eq!(result.title, "Rust in the news");
                assert_eq!(result.content, "Something happened.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn time_range_is_mapped_to_bing_intervals() {
        let mut query = Query::parse("rust", &EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Day);
        let url = engine().build_request(&query, 1).unwrap().url;
        assert!(url.contains("qft=interval%3D%224%22"), "{url}");
        query.time_range = Some(TimeRange::Year);
        let url = engine().build_request(&query, 1).unwrap().url;
        assert!(!url.contains("qft"));
    }
}
