//! SolidTorrents engine, ported from searx/engines/solidtorrents.py.
// engine-id: solidtorrents

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "solid",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://solidtorrents.to";

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

/// SolidTorrents, the HTML search page.
pub struct SolidTorrents {
    meta: EngineMeta,
}

impl SolidTorrents {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for SolidTorrents {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("page", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
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
        for item in select_elements(&html, "li.search-result")? {
            // Upstream ignores anime entries that are not actually torrents.
            let Some(torrent) = attr_of(item, "a.dl-torrent", "href") else {
                continue;
            };
            let Some(magnet) = attr_of(item, "a.dl-magnet", "href") else {
                continue;
            };
            let Some(title_element) = child(item, "h5.title") else {
                continue;
            };
            let title = text(title_element);
            if title.is_empty() {
                continue;
            }
            let Some(href) = attr_of(item, "h5.title a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let stats = children(item, "div.stats div");
            if stats.len() < 5 {
                continue;
            }
            let filesize = text(stats[1]);
            let leech = text(stats[2]);
            let seed = text(stats[3]);
            let date = text(stats[4]);
            let category = text_of(item, "a.category").unwrap_or_default();
            let torrent_url = absolute_url(&torrent, base).unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .metadata("category", category)
                .metadata("seed", seed)
                .metadata("leech", leech)
                .metadata("filesize", filesize)
                .metadata("magnetlink", magnet)
                .metadata("torrentfile", torrent_url);
            if let Ok(parsed) = chrono::NaiveDate::parse_from_str(date.trim(), "%b %d, %Y") {
                builder = builder.published_date(parsed.format("%Y-%m-%dT00:00:00").to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<ul>
      <li class="search-result">
        <h5 class="title"><a href="/torrent/abc">Ubuntu 24.04</a></h5>
        <a class="dl-torrent" href="/download/abc.torrent">torrent</a>
        <a class="dl-magnet" href="magnet:?xt=urn:btih:ABC">magnet</a>
        <a class="category" href="/c/software">Software</a>
        <div class="stats">
          <div>2023</div><div>5.8 GB</div><div>10</div><div>120</div><div>Oct 24, 2023</div>
        </div>
      </li>
      <li class="search-result">
        <h5 class="title"><a href="/torrent/def">No magnet</a></h5>
        <a class="dl-torrent" href="/download/def.torrent">torrent</a>
        <div class="stats"><div>2023</div></div>
      </li>
    </ul>"##;

    fn engine() -> Arc<SolidTorrents> {
        SolidTorrents::from_config(&EngineConfig::new(
            "solidtorrents",
            "solidtorrents",
            "solid",
            &["files"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_result_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://solidtorrents.to/torrent/abc");
                assert_eq!(result.title, "Ubuntu 24.04");
                assert_eq!(result.metadata["category"], "Software");
                assert_eq!(result.metadata["seed"], "120");
                assert_eq!(result.metadata["leech"], "10");
                assert_eq!(result.metadata["filesize"], "5.8 GB");
                assert_eq!(result.metadata["magnetlink"], "magnet:?xt=urn:btih:ABC");
                assert_eq!(
                    result.metadata["torrentfile"],
                    "https://solidtorrents.to/download/abc.torrent"
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2023-10-24T00:00:00")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_search_url() {
        let query = Query::parse("ubuntu iso", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://solidtorrents.to/search?q=ubuntu%20iso&page=2"
        );
    }
}
