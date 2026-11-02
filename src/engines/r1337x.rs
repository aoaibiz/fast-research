//! 1337x engine, ported from `searx/engines/1337x.py`.
// engine-id: 1337x

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://1337x.to/";

/// 1337x torrent search.
pub struct _1337x {
    meta: EngineMeta,
}

impl _1337x {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for _1337x {
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
        let url = format!("{base}/search/{}/{pageno}/", encode(&query.query_text()));
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let mut results = Vec::new();
        for row in select_elements(&html, "table.table-list tbody tr")? {
            // Upstream takes the second <a> inside the name cell (the title link).
            let link = child(row, "td.name a:nth-of-type(2)");
            let Some(link) = link else { continue };
            let Some(url) = attr(link, "href").and_then(|href| absolute_url(&href, base)) else {
                continue;
            };
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let seed = text_of(row, "td.seeds").unwrap_or_default();
            let leech = text_of(row, "td.leeches").unwrap_or_default();
            let filesize = text_of(row, "td.size").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title);
            if !seed.is_empty() {
                builder = builder.metadata("seed", seed);
            }
            if !leech.is_empty() {
                builder = builder.metadata("leech", leech);
            }
            if !filesize.is_empty() {
                builder = builder.metadata("filesize", filesize);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_torrent_rows() {
        let html = r#"
        <table class="table-list">
          <tbody>
            <tr>
              <td class="coll-1 name"><a href="/torrent/123/ubuntu" class="icon"></a><a href="/torrent/123/ubuntu">Ubuntu 24.04 ISO</a></td>
              <td class="coll-2 seeds">120</td>
              <td class="coll-3 leeches">10</td>
              <td class="coll-4 size">5.8 GB</td>
            </tr>
            <tr>
              <td class="coll-1 name"><a href="/torrent/456" class="icon"></a><a href="/torrent/456">Debian</a></td>
              <td class="coll-2 seeds"></td>
              <td class="coll-3 leeches">1</td>
              <td class="coll-4 size">700 MB</td>
            </tr>
            <tr><td class="coll-1 name"><a href="javascript:void(0)">broken</a></td></tr>
          </tbody>
        </table>"#;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let engine =
            _1337x::from_config(&EngineConfig::new("1337x", "1337x", "", &["files"])).unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        // The javascript: row is skipped (non-http url).
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1337x.to/torrent/123/ubuntu");
                assert_eq!(result.title, "Ubuntu 24.04 ISO");
                assert_eq!(result.metadata["seed"], "120");
                assert_eq!(result.metadata["leech"], "10");
                assert_eq!(result.metadata["filesize"], "5.8 GB");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://1337x.to/torrent/456");
                assert!(!result.metadata.contains_key("seed"));
                assert_eq!(result.metadata["leech"], "1");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_search_url() {
        let engine =
            _1337x::from_config(&EngineConfig::new("1337x", "1337x", "", &["files"])).unwrap();
        let query = Query::parse("ubuntu iso", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://1337x.to/search/ubuntu%20iso/2/");
    }
}
