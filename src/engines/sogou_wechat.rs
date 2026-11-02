//! Sogou WeChat engine, ported from searx/engines/sogou_wechat.py.
// engine-id: sogou_wechat

use std::sync::Arc;

use chrono::TimeZone;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "sogouw",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://weixin.sogou.com";

/// Sogou WeChat article search.
pub struct SogouWechat {
    meta: EngineMeta,
}

impl SogouWechat {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for SogouWechat {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("query", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("type", "2".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/weixin?{}",
            self.base(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base().to_string();
        let mut results = Vec::new();
        for item in select_elements(&html, "li[id*='sogou_vr_']")? {
            let Some(link) = child(item, "h3 a") else {
                continue;
            };
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let href = if href.starts_with("/link?url=") {
                format!("{base}{href}")
            } else {
                href
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http(&url) {
                continue;
            }
            let content = text_of(item, "p.txt-info")
                .or_else(|| text_of(item, "p[class*='txt-info']"))
                .unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(src) = attr_of(item, "div.img-box a img", "src") {
                let src = if src.starts_with("//") {
                    format!("https:{src}")
                } else {
                    src
                };
                if is_http(&src) {
                    builder = builder.thumbnail(src);
                }
            }
            builder = with_published_date(builder, item);
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The date is encoded in a timeConvert script inside the item.
fn with_published_date(mut builder: ResultBuilder, item: scraper::ElementRef<'_>) -> ResultBuilder {
    for script in children(item, "script") {
        let body = script.text().collect::<Vec<_>>().join(" ");
        if !body.contains("timeConvert(") {
            continue;
        }
        if let Some(seconds) = timestamp_of(&body) {
            if let Some(date) = chrono::Local.timestamp_opt(seconds, 0).single() {
                builder = builder.published_date(date.format("%Y-%m-%dT%H:%M:%S").to_string());
            }
        }
        break;
    }
    builder
}

/// The first number after the timeConvert call.
fn timestamp_of(body: &str) -> Option<i64> {
    let start = body.find("timeConvert(")? + "timeConvert(".len();
    let digits: String = body[start..]
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body><ul>
      <li id="sogou_vr_11002601_1">
        <div class="img-box"><a href="/link?url=acc"><img src="//img.example.com/thumb.jpg"></a></div>
        <h3><a href="/link?url=abc">WeChat article title</a></h3>
        <p class="txt-info">Some summary text.</p>
        <script>var t = timeConvert('1700000000');</script>
      </li>
      <li id="sogou_vr_11002601_2">
        <h3><a href="https://mp.weixin.qq.com/s/xyz">Second article</a></h3>
        <p class="txt-info extra">Second summary.</p>
      </li>
      <li id="sogou_vr_11002601_3"><h3><a href="/link?url=empty"></a></h3></li>
    </ul></body></html>"##;

    fn engine() -> Arc<SogouWechat> {
        SogouWechat::from_config(&EngineConfig::new(
            "sogou wechat",
            "sogou_wechat",
            "sogouw",
            &["news"],
        ))
        .unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_article_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        let expected = chrono::Local
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .unwrap()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected an article")
        };
        assert_eq!(first.title, "WeChat article title");
        assert_eq!(first.url, "https://weixin.sogou.com/link?url=abc");
        assert_eq!(first.content, "Some summary text.");
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://img.example.com/thumb.jpg")
        );
        assert_eq!(first.published_date.as_deref(), Some(expected.as_str()));
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected an article")
        };
        assert_eq!(second.url, "https://mp.weixin.qq.com/s/xyz");
        assert_eq!(second.content, "Second summary.");
        assert!(second.published_date.is_none());
    }

    #[test]
    fn constructs_the_search_url() {
        let request = engine().build_request(&query("rust"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://weixin.sogou.com/weixin?query=rust&page=2&type=2"
        );
    }

    #[test]
    fn a_page_without_items_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(
            engine()
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
