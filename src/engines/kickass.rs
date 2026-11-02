//! Kickass engine, ported from searx/engines/kickass.py.
// engine-id: kickass

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "kc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://kickasstorrents.to";

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

/// Upstream's int_or_zero.
fn int_or_zero(value: &str) -> i64 {
    value.trim().replace(',', "").parse::<i64>().unwrap_or(0)
}

/// Kickass Torrents, the HTML search page.
pub struct Kickass {
    meta: EngineMeta,
}

impl Kickass {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Kickass {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/usearch/{}/{}/",
            site_base(&self.meta),
            encode(&query.query_text()),
            pageno
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = site_base(&self.meta);
        let mut rows = Vec::new();
        for row in select_elements(&html, "table.data tr")? {
            if child(row, "a").is_some() {
                rows.push(row);
            }
        }
        let mut results: Vec<(i64, EngineItem)> = Vec::new();
        // Upstream skips the first matching row, the table header.
        for row in rows.iter().skip(1) {
            let row = *row;
            let Some(link) = child(row, "a.cellMainLink") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let seed = text_of(row, "td.green").unwrap_or_default();
            let leech = text_of(row, "td.red").unwrap_or_default();
            let filesize = text_of(row, "td.nobr").unwrap_or_default();
            let content = text_of(row, "span.font11px.lightgrey.block").unwrap_or_default();
            let builder = ResultBuilder::new(url, title)
                .content(content)
                .metadata("seed", seed.clone())
                .metadata("leech", leech.clone())
                .metadata("filesize", filesize);
            results.push((int_or_zero(&seed), builder.build()));
        }
        results.sort_by_key(|item| std::cmp::Reverse(item.0));
        Ok(results.into_iter().map(|(_, item)| item).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<table class="data">
      <tr><th><a href="/">Browse</a></th><th>Name</th><th>Size</th><th>Seed</th><th>Leech</th></tr>
      <tr>
        <td><a class="cellMainLink" href="/ubuntu-torrent-123.html">Ubuntu 24.04</a></td>
        <td><span class="font11px lightgrey block">A free operating system</span></td>
        <td class="nobr">5.8 GB</td>
        <td class="green">120</td>
        <td class="red">10</td>
      </tr>
      <tr>
        <td><a class="cellMainLink" href="javascript:void(0)">Broken</a></td>
        <td><span class="font11px lightgrey block">no link</span></td>
        <td class="nobr">1 MB</td>
        <td class="green">999</td>
        <td class="red">1</td>
      </tr>
      <tr>
        <td><a class="cellMainLink" href="/debian-torrent-456.html">Debian 12</a></td>
        <td><span class="font11px lightgrey block">The universal OS</span></td>
        <td class="nobr">700 MB</td>
        <td class="green">5</td>
        <td class="red">1</td>
      </tr>
    </table>"##;

    fn engine() -> Arc<Kickass> {
        Kickass::from_config(&EngineConfig::new("kickass", "kickass", "kc", &["files"])).unwrap()
    }

    #[test]
    fn parses_and_sorts_the_result_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.url,
                    "https://kickasstorrents.to/ubuntu-torrent-123.html"
                );
                assert_eq!(result.title, "Ubuntu 24.04");
                assert_eq!(result.content, "A free operating system");
                assert_eq!(result.metadata["seed"], "120");
                assert_eq!(result.metadata["leech"], "10");
                assert_eq!(result.metadata["filesize"], "5.8 GB");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Debian 12");
                assert_eq!(result.metadata["seed"], "5");
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
            "https://kickasstorrents.to/usearch/ubuntu%20iso/2/"
        );
    }
}
