//! iQiyi engine, ported from searx/engines/iqiyi.py.
// engine-id: iqiyi

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
    shortcut: "iq",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://mesh.if.iqiyi.com";
const PAGE_SIZE: u32 = 25;

/// The iQiyi web search API.
pub struct Iqiyi {
    meta: EngineMeta,
}

impl Iqiyi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn endpoint(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Iqiyi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("key", query.query_text()),
            ("pageNum", pageno.max(1).to_string()),
            ("pageSize", PAGE_SIZE.to_string()),
        ];
        // Upstream has no filter for the year range.
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "1",
                TimeRange::Week => "2",
                TimeRange::Month => "3",
                TimeRange::Year => "",
            };
            if !value.is_empty() {
                pairs.push(("sitePublishDate", value.to_string()));
            }
        }
        Ok(EngineRequest::get(format!(
            "{}/portal/lw/search/homePageV3?{}",
            self.endpoint(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(templates) = json_path(&json, "data.templates").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in templates {
            let Some(album) = entry.get("albumInfo") else {
                continue;
            };
            if let Some(videos) = album.get("videos").and_then(|value| value.as_array()) {
                for video in videos {
                    if let Some(item) = video_result(video, album) {
                        results.push(item);
                    }
                }
            } else if let Some(item) = video_result(album, album) {
                results.push(item);
            }
        }
        Ok(results)
    }
}

/// One entry of the templates list, upstream _result().
fn video_result(video: &serde_json::Value, album: &serde_json::Value) -> Option<EngineItem> {
    let url = json_str(video, "pageUrl").replace("http://", "https://");
    let title = json_str(video, "title");
    let http = url.starts_with("http://") || url.starts_with("https://");
    if title.is_empty() || !http {
        return None;
    }
    let mut builder = ResultBuilder::new(url, title)
        .template(Template::Videos)
        .content(json_str(album, "brief.value"))
        .published_date(published_date(&json_str(album, "releaseTime.value")))
        .metadata("length", length(video));
    let thumbnail = json_str(album, "img");
    if !thumbnail.is_empty() {
        builder = builder.thumbnail(thumbnail);
    }
    Some(builder.build())
}

/// The duration arrives in milliseconds.
fn length(video: &serde_json::Value) -> String {
    match json_path(video, "duration").and_then(serde_json::Value::as_i64) {
        Some(milliseconds) => format_duration(milliseconds.max(0) / 1000),
        None => String::new(),
    }
}

fn format_duration(seconds: i64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Upstream parses releaseTime.value with datetime.fromisoformat.
fn published_date(value: &str) -> String {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return date.to_rfc3339();
    }
    if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return date.format("%Y-%m-%dT%H:%M:%S").to_string();
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%dT00:00:00").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "data": {"templates": [
        {"albumInfo": {
          "title": "Album", "pageUrl": "http://www.iqiyi.com/v_album.html",
          "img": "https://pic.iqiyi.com/album.jpg",
          "brief": {"value": "A drama"},
          "releaseTime": {"value": "2023-05-01T10:00:00+08:00"},
          "videos": [{"title": "Episode 1", "pageUrl": "http://www.iqiyi.com/v_ep1.html", "duration": 2700000}]
        }},
        {"albumInfo": {
          "title": "Single video", "pageUrl": "https://www.iqiyi.com/v_single.html",
          "img": "https://pic.iqiyi.com/single.jpg",
          "brief": {"value": "A film"},
          "releaseTime": {"value": "2024-01-02"},
          "duration": 5400000
        }}
      ]}
    }"##;

    fn engine() -> Arc<Iqiyi> {
        Iqiyi::from_config(&EngineConfig::new("iqiyi", "iqiyi", "iq", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_albums_and_single_videos() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match (&items[0], &items[1]) {
            (EngineItem::Result(first), EngineItem::Result(second)) => {
                assert_eq!(first.title, "Episode 1");
                assert_eq!(first.url, "https://www.iqiyi.com/v_ep1.html");
                assert_eq!(first.template, Template::Videos);
                assert_eq!(first.content, "A drama");
                assert_eq!(
                    first.thumbnail.as_deref(),
                    Some("https://pic.iqiyi.com/album.jpg")
                );
                assert_eq!(
                    first.published_date.as_deref(),
                    Some("2023-05-01T10:00:00+08:00")
                );
                assert_eq!(
                    first.metadata.get("length").map(String::as_str),
                    Some("45:00")
                );
                assert_eq!(second.title, "Single video");
                assert_eq!(second.url, "https://www.iqiyi.com/v_single.html");
                assert_eq!(second.content, "A film");
                assert_eq!(
                    second.published_date.as_deref(),
                    Some("2024-01-02T00:00:00")
                );
                assert_eq!(
                    second.metadata.get("length").map(String::as_str),
                    Some("01:30:00")
                );
            }
            _ => panic!("expected two results"),
        }
    }

    #[test]
    fn request_forwards_paging_and_the_date_filter() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://mesh.if.iqiyi.com/portal/lw/search/homePageV3?key=rust&pageNum=2&pageSize=25"
        );
        let mut query = query();
        query.time_range = Some(TimeRange::Week);
        assert!(
            engine()
                .build_request(&query, 1)
                .unwrap()
                .url
                .contains("sitePublishDate=2")
        );
        query.time_range = Some(TimeRange::Year);
        assert!(
            !engine()
                .build_request(&query, 1)
                .unwrap()
                .url
                .contains("sitePublishDate")
        );
    }

    #[test]
    fn missing_containers_and_broken_bodies() {
        let response = EngineResponse::new(200, BASE_URL, b"{\"data\": {}}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
        let response = EngineResponse::new(200, BASE_URL, b"not json".to_vec());
        assert!(matches!(
            engine().parse(&response, &query()),
            Err(EngineError::Parse(_))
        ));
    }
}
