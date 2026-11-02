//! Niconico engine, ported from searx/engines/niconico.py.
// engine-id: niconico

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "nico",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.nicovideo.jp";
const EMBED_URL: &str = "https://embed.nicovideo.jp";

/// The Niconico video search.
pub struct Niconico {
    meta: EngineMeta,
}

impl Niconico {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Niconico {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![("page", pageno.max(1).to_string())];
        if let Some(range) = query.time_range {
            let days = match range {
                TimeRange::Day => 1,
                TimeRange::Week => 7,
                TimeRange::Month => 30,
                TimeRange::Year => 365,
            };
            let start = chrono::Utc::now()
                .date_naive()
                .checked_sub_days(chrono::Days::new(days))
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            pairs.push(("start", start));
        }
        Ok(EngineRequest::get(format!(
            "{}/search/{}?{}",
            self.base_url(),
            query.query_text(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let mut results = Vec::new();
        for item in select_elements(&html, "li[data-video-item]")? {
            let Some(href) = attr_of(item, "a.itemThumbWrap", "href") else {
                continue;
            };
            let video_id = video_id(&href);
            let title = text_of(item, "p.itemTitle a").unwrap_or_default();
            if video_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{}/watch/{video_id}", self.base_url());
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(attr_of(item, "p.itemDescription", "title").unwrap_or_default())
                .metadata("iframe_src", format!("{EMBED_URL}/watch/{video_id}"));
            if let Some(length) = length(&text_of(item, "span.videoLength").unwrap_or_default()) {
                builder = builder.metadata("length", length);
            }
            if let Some(date) =
                published_date(&text_of(item, "p.itemTime span.time").unwrap_or_default())
            {
                builder = builder.published_date(date);
            }
            let thumbnail = attr_of(item, "img.thumb", "src").unwrap_or_default();
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The id of the watched video, upstream splits the watch link on ? and /.
fn video_id(href: &str) -> String {
    href.split('?')
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Upstream parses the length with strptime %M:%S, longer videos are dropped.
fn length(value: &str) -> Option<String> {
    let (minutes, seconds) = value.trim().split_once(':')?;
    if seconds.contains(':') {
        return None;
    }
    let minutes: u32 = minutes.parse().ok()?;
    let seconds: u32 = seconds.parse().ok()?;
    if minutes > 59 || seconds > 59 {
        return None;
    }
    Some(format!("{minutes:02}:{seconds:02}"))
}

/// Upstream parses the upload time with strptime %Y/%m/%d %H:%M.
fn published_date(value: &str) -> Option<String> {
    chrono::NaiveDateTime::parse_from_str(value.trim(), "%Y/%m/%d %H:%M")
        .ok()
        .map(|date| date.format("%Y-%m-%dT%H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body><ul class="searchResult">
      <li data-video-item>
        <a class="itemThumbWrap" href="/watch/sm12345?from=search">
          <img class="thumb" src="https://nicovideo.cdn.nimg.jp/thumb/sm12345">
          <span class="videoLength">05:30</span>
        </a>
        <p class="itemTitle"><a href="/watch/sm12345">Rust tutorial</a></p>
        <p class="itemDescription" title="A description">ignored body</p>
        <p class="itemTime"><span class="time">2024/05/01 12:30</span></p>
      </li>
    </ul></body></html>"##;

    fn engine() -> Arc<Niconico> {
        Niconico::from_config(&EngineConfig::new(
            "niconico",
            "niconico",
            "nico",
            &["videos"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_video_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust tutorial");
                assert_eq!(result.url, "https://www.nicovideo.jp/watch/sm12345");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "A description");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://nicovideo.cdn.nimg.jp/thumb/sm12345")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:30:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("05:30")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://embed.nicovideo.jp/watch/sm12345")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_missing_listing_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }

    #[test]
    fn paging_and_time_range_are_forwarded() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(request.url, "https://www.nicovideo.jp/search/rust?page=2");
        let mut query = query();
        query.time_range = Some(TimeRange::Month);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.nicovideo.jp/search/rust?page=1&start=")
        );
    }
}
