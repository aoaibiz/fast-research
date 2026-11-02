//! Nyaa engine, ported from searx/engines/nyaa.py.
// engine-id: nyaa

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "nt",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://nyaa.si";

/// The configured base without a trailing slash.
fn site_base(meta: &EngineMeta) -> &str {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Parse a counter cell, treating a missing or broken value as zero.
fn int_or_zero(value: Option<String>) -> i64 {
    value
        .unwrap_or_default()
        .trim()
        .replace(',', "")
        .parse::<i64>()
        .unwrap_or(0)
}

/// Nyaa.si, the anime BitTorrent tracker.
pub struct Nyaa {
    meta: EngineMeta,
}

impl Nyaa {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Nyaa {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![("q", query.query_text()), ("p", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/?{}",
            site_base(&self.meta),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = site_base(&self.meta);
        let mut results = Vec::new();
        for row in select_elements(&html, "table.torrent-list tbody tr")? {
            let mut links = children(row, "td:nth-child(2) a");
            let Some(page_link) = links.pop() else {
                continue;
            };
            let Some(href) = attr(page_link, "href") else {
                continue;
            };
            let title = text(page_link);
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let category = children(row, "td:nth-child(1) a")
                .first()
                .and_then(|link| attr(*link, "title"))
                .unwrap_or_default();
            let mut magnet = String::new();
            let mut torrent = String::new();
            for link in children(row, "td:nth-child(3) a") {
                if let Some(link) = attr(link, "href") {
                    if link.starts_with("magnet") {
                        magnet = link;
                    } else {
                        torrent = absolute_url(&link, base).unwrap_or_default();
                    }
                }
            }
            let seed = int_or_zero(text_of(row, "td:nth-child(6)"));
            let leech = int_or_zero(text_of(row, "td:nth-child(7)"));
            let downloads = int_or_zero(text_of(row, "td:nth-child(8)"));
            let filesize = text_of(row, "td:nth-child(4)").unwrap_or_default();
            let builder = ResultBuilder::new(url, title)
                .content(format!(
                    "Category: \"{}\". Downloaded {} times.",
                    category, downloads
                ))
                .metadata("seed", seed.to_string())
                .metadata("leech", leech.to_string())
                .metadata("filesize", filesize)
                .metadata("magnetlink", magnet)
                .metadata("torrentfile", torrent);
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<table class="torrent-list">
      <thead><tr><th>Category</th><th>Name</th><th>Links</th><th>Size</th><th>Date</th><th>Seeders</th><th>Leechers</th><th>Downloads</th></tr></thead>
      <tbody>
        <tr>
          <td><a href="/?c=1_2" title="Anime - English-translated">Anime</a></td>
          <td><a href="/view/123">[Group] Show - 01</a></td>
          <td><a href="magnet:?xt=urn:btih:ABC&dn=Show">magnet</a><a href="/download/123.torrent">torrent</a></td>
          <td>1.2 GiB</td>
          <td>2024-01-01</td>
          <td>42</td>
          <td>3</td>
          <td>100</td>
        </tr>
        <tr>
          <td><a href="/?c=1_2" title="Anime">Anime</a></td>
          <td><a href="javascript:void(0)">Broken</a></td>
          <td></td><td></td><td></td><td></td><td></td><td></td>
        </tr>
      </tbody>
    </table>"##;

    fn engine() -> Arc<Nyaa> {
        Nyaa::from_config(&EngineConfig::new("nyaa", "nyaa", "nt", &["files"])).unwrap()
    }

    #[test]
    fn parses_the_torrent_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("show", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://nyaa.si/view/123");
                assert_eq!(result.title, "[Group] Show - 01");
                assert_eq!(
                    result.content,
                    "Category: \"Anime - English-translated\". Downloaded 100 times."
                );
                assert_eq!(result.metadata["seed"], "42");
                assert_eq!(result.metadata["leech"], "3");
                assert_eq!(result.metadata["filesize"], "1.2 GiB");
                assert_eq!(
                    result.metadata["magnetlink"],
                    "magnet:?xt=urn:btih:ABC&dn=Show"
                );
                assert_eq!(
                    result.metadata["torrentfile"],
                    "https://nyaa.si/download/123.torrent"
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_search_url() {
        let query = Query::parse("one piece", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://nyaa.si/?q=one%20piece&p=2");
    }
}
