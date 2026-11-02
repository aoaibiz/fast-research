//! Bandcamp engine, ported from `searx/engines/bandcamp.py`.
// engine-id: bandcamp

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music"],
    shortcut: "bc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://bandcamp.com/";
const IFRAME_SRC: &str = "https://bandcamp.com/EmbeddedPlayer/{kind}={result_id}/size=large/bgcol=000/linkcol=fff/artwork=small";

/// Bandcamp.
pub struct Bandcamp {
    meta: EngineMeta,
}

impl Bandcamp {
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

impl Engine for Bandcamp {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}search?{}&page={}",
            self.base_url(),
            query_string(&[("q", query.query_text())]),
            pageno.max(1)
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
        for element in select_elements(&html, "li.searchresult")? {
            let Some(link) = child(element, "div.itemurl a") else {
                continue;
            };
            let href = attr(link, "href");
            let mut url = text(link);
            if url.is_empty() {
                url = href
                    .as_deref()
                    .and_then(|href| absolute_url(href, &base))
                    .unwrap_or_default();
            }
            let title = text_of(element, "div.heading a").unwrap_or_default();
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let content = text_of(element, "div.subhead").unwrap_or_default();
            let released = text_of(element, "div.released")
                .map(|value| value.replacen("released ", "", 1))
                .unwrap_or_default();
            let thumbnail = attr_of(element, "div.art img", "src").unwrap_or_default();
            let item_type = text_of(element, "div.itemtype")
                .unwrap_or_default()
                .to_lowercase();
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .thumbnail(thumbnail)
                .published_date(released)
                .metadata("type", item_type.clone());
            if let (Some(href), true) = (
                href.as_deref(),
                item_type == "album" || item_type == "track",
            ) {
                if let Some(result_id) = search_item_id(href, &base) {
                    let iframe = IFRAME_SRC
                        .replace("{kind}", &item_type)
                        .replace("{result_id}", &result_id);
                    builder = builder.metadata("iframe_src", iframe);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The `search_item_id` query parameter of a result link.
fn search_item_id(href: &str, base: &str) -> Option<String> {
    let url = url::Url::parse(href)
        .or_else(|_| url::Url::parse(base).and_then(|base| base.join(href)))
        .ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "search_item_id")
        .map(|(_, value)| value.to_string())
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <html><body>
      <ul>
        <li class="searchresult data-search">
          <div class="art"><img src="https://f4.bcbits.com/img/a123_16.jpg"></div>
          <div class="heading"><a href="https://artist.bandcamp.com/album/foo?from=search&amp;search_item_id=12345">Foo</a></div>
          <div class="subhead">by Some Artist</div>
          <div class="released">released January 1, 2024</div>
          <div class="itemtype">ALBUM</div>
          <div class="itemurl"><a href="https://artist.bandcamp.com/album/foo?from=search&amp;search_item_id=12345">https://artist.bandcamp.com/album/foo</a></div>
        </li>
      </ul>
    </body></html>"#;

    fn engine() -> Arc<Bandcamp> {
        Bandcamp::from_config(&EngineConfig::new("bandcamp", "bandcamp", "bc", &["music"])).unwrap()
    }

    #[test]
    fn parses_album_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("foo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Foo");
                assert_eq!(result.url, "https://artist.bandcamp.com/album/foo");
                assert_eq!(result.content, "by Some Artist");
                assert_eq!(result.published_date.as_deref(), Some("January 1, 2024"));
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some(
                        "https://bandcamp.com/EmbeddedPlayer/album=12345/size=large/bgcol=000/linkcol=fff/artwork=small"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("foo bar", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://bandcamp.com/search?q=foo%20bar&page=2"
        );
    }
}
