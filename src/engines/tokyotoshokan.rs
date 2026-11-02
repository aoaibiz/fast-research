//! Tokyo Toshokan engine, ported from searx/engines/tokyotoshokan.py.
// engine-id: tokyotoshokan

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "tt",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const BASE_URL: &str = "https://www.tokyotosho.info";

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

/// Tokyo Toshokan, a BitTorrent library for Japanese media.
pub struct TokyoToshokan {
    meta: EngineMeta,
}

impl TokyoToshokan {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for TokyoToshokan {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> =
            vec![("page", pageno.to_string()), ("terms", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}/search.php?{}",
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
        let rows = select_elements(&html, "table.listing tr.category_0")?;
        // Each result is two rows; an odd count means the layout changed.
        if rows.is_empty() || rows.len() % 2 != 0 {
            return Ok(Vec::new());
        }
        // Upstream's torrent size pattern, tolerating the space some rows use.
        let size_re = regex::Regex::new(r"(?i)[\d.]+(?:\s*(?:T|G|M))?B")
            .map_err(|error| EngineError::Parse(format!("bad size pattern: {error}")))?;
        let mut results = Vec::new();
        for pair in rows.chunks(2) {
            let name_row = pair[0];
            let info_row = pair[1];
            let links = children(name_row, "td.desc-top a");
            let Some(last) = links.last().copied() else {
                continue;
            };
            let Some(href) = attr(last, "href") else {
                continue;
            };
            let title = text(last);
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title);
            if links.len() == 2 {
                if let Some(magnet) = attr(links[0], "href") {
                    if magnet.starts_with("magnet") {
                        builder = builder.metadata("magnetlink", magnet);
                    }
                }
            }
            if let Some(desc) = text_of(info_row, "td.desc-bot") {
                for part in desc.split('|') {
                    let part = part.trim();
                    if let Some(size) = part.strip_prefix("Size:") {
                        if let Some(found) = size_re.find(size.trim()) {
                            builder = builder.metadata("filesize", found.as_str());
                        }
                    } else if let Some(date) = part.strip_prefix("Date:") {
                        let date = date.trim().trim_end_matches("UTC").trim();
                        if let Ok(parsed) =
                            chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M")
                        {
                            builder = builder
                                .published_date(parsed.format("%Y-%m-%dT%H:%M:%S").to_string());
                        }
                    } else if part.starts_with("Comment:") {
                        builder = builder.content(part);
                    }
                }
            }
            let stats = children(info_row, "td.stats span");
            if stats.len() == 3 {
                builder = builder
                    .metadata("seed", text(stats[0]))
                    .metadata("leech", text(stats[1]));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<table class="listing">
      <tr class="category_0">
        <td class="desc-top"><a href="magnet:?xt=urn:btih:ABC">magnet</a><a href="https://www.tokyotosho.info/details.php?id=1">[Group] Title</a></td>
      </tr>
      <tr class="category_0">
        <td class="desc-bot">Size: 1.4GB | Date: 2016-02-21 21:44 UTC | Comment: hello</td>
        <td class="stats"><span>10</span><span>2</span><span>0</span></td>
      </tr>
    </table>"##;

    fn engine() -> Arc<TokyoToshokan> {
        TokyoToshokan::from_config(&EngineConfig::new(
            "tokyotoshokan",
            "tokyotoshokan",
            "tt",
            &["files"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_two_row_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("title", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.tokyotosho.info/details.php?id=1");
                assert_eq!(result.title, "[Group] Title");
                assert_eq!(result.content, "Comment: hello");
                assert_eq!(result.metadata["filesize"], "1.4GB");
                assert_eq!(result.metadata["magnetlink"], "magnet:?xt=urn:btih:ABC");
                assert_eq!(result.metadata["seed"], "10");
                assert_eq!(result.metadata["leech"], "2");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2016-02-21T21:44:00")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn odd_row_counts_and_the_request_are_handled() {
        let odd = r##"<table class="listing">
          <tr class="category_0"><td class="desc-top"><a href="https://www.tokyotosho.info/details.php?id=2">One row</a></td></tr>
        </table>"##;
        let response = EngineResponse::new(200, BASE_URL, odd.as_bytes().to_vec());
        let query = Query::parse("one piece", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.tokyotosho.info/search.php?page=2&terms=one%20piece"
        );
    }
}
