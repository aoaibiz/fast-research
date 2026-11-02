//! BTDigg engine, ported from searx/engines/btdigg.py.
// engine-id: btdigg

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "bt",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://btdig.com";

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// BTDigg, the HTML torrent search page.
pub struct Btdigg {
    meta: EngineMeta,
}

impl Btdigg {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Btdigg {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        // Upstream pages start at zero: page 1 is p=0.
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("p", pageno.saturating_sub(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{base}/search?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut results = Vec::new();
        for item in select_elements(&html, "div.one_result")? {
            let Some(anchor) = child(item, "div.torrent_name a") else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            let title = text(anchor);
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            let Some(excerpt) = child(item, "div.torrent_excerpt") else {
                continue;
            };
            let lines: Vec<String> = excerpt
                .text()
                .map(clean)
                .filter(|line| !line.is_empty())
                .collect();
            let content = lines.join(" | ");
            let Some(filesize) = text_of(item, "span.torrent_size") else {
                continue;
            };
            let files = text_of(item, "span.torrent_files").unwrap_or_else(|| "1".to_string());
            let Some(magnet) = attr_of(item, "div.torrent_magnet a", "href") else {
                continue;
            };
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .metadata("filesize", filesize)
                .metadata("magnetlink", magnet);
            if let Ok(count) = files.trim().parse::<i64>() {
                builder = builder.metadata("files", count.to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<div class="one_result">
        <div class="torrent_name"><a href="/torrent/abc">Ubuntu 24.04</a></div>
        <div class="torrent_excerpt">Ubuntu 24.04<br>6.1 GB<br>2 files</div>
        <span class="torrent_size">6.1 GB</span>
        <span class="torrent_files">2</span>
        <div class="torrent_magnet"><a href="magnet:?xt=urn:btih:ABC">magnet</a></div>
      </div>
      <div class="one_result">
        <div class="torrent_name"><a href="javascript:alert(1)">Broken</a></div>
      </div>"##;

    fn engine() -> Arc<Btdigg> {
        Btdigg::from_config(&EngineConfig::new("btdigg", "btdigg", "bt", &["files"])).unwrap()
    }

    #[test]
    fn parses_the_result_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.url, "https://btdig.com/torrent/abc");
        assert_eq!(result.title, "Ubuntu 24.04");
        assert_eq!(result.content, "Ubuntu 24.04 | 6.1 GB | 2 files");
        assert_eq!(
            result.metadata.get("filesize").map(String::as_str),
            Some("6.1 GB")
        );
        assert_eq!(result.metadata.get("files").map(String::as_str), Some("2"));
        assert_eq!(
            result.metadata.get("magnetlink").map(String::as_str),
            Some("magnet:?xt=urn:btih:ABC")
        );
    }

    #[test]
    fn builds_the_zero_based_page_url() {
        let query = Query::parse("ubuntu iso", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert_eq!(request.url, "https://btdig.com/search?q=ubuntu%20iso&p=2");
    }
}
