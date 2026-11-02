//! Bilibili engine, ported from searx/engines/bilibili.py.
// engine-id: bilibili

use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveTime, Utc};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "bil",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.bilibili.com/x/web-interface/search/type";
const PAGE_SIZE: u32 = 20;
const IFRAME_PREFIX: &str = "https://player.bilibili.com/player.html?aid=";
// Upstream randomises this cookie; a fixed value keeps requests cacheable.
const BUVID3: &str = "7f3a9c2e5b8d4106infoc";

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Parse MM:SS or HH:MM:SS into seconds, like utils.parse_duration_string.
fn duration_seconds(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.trim().split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut seconds = 0i64;
    for part in parts {
        seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    Some(seconds)
}

/// Calendar day bounds in China Standard Time, Bilibili's time range semantics.
fn day_bounds(range: TimeRange) -> Option<(i64, i64)> {
    let days: u64 = match range {
        TimeRange::Day => 0,
        TimeRange::Week => 6,
        TimeRange::Month => 29,
        TimeRange::Year => 364,
    };
    let today = (Utc::now() + Duration::hours(8)).date_naive();
    let begin_day = today.checked_sub_days(chrono::Days::new(days))?;
    let begin = begin_day
        .and_time(NaiveTime::from_hms_opt(0, 0, 0)?)
        .and_utc()
        .timestamp();
    let end = today
        .and_time(NaiveTime::from_hms_opt(23, 59, 59)?)
        .and_utc()
        .timestamp();
    Some((begin - 28_800, end - 28_800))
}

/// Bilibili, the video search API.
pub struct Bilibili {
    meta: EngineMeta,
}

impl Bilibili {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Bilibili {
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
        let mut pairs: Vec<(&str, String)> = vec![
            ("__refresh__", "true".to_string()),
            ("page", pageno.max(1).to_string()),
            ("page_size", PAGE_SIZE.to_string()),
            ("single_column", "0".to_string()),
            ("keyword", query.query_text()),
            ("search_type", "video".to_string()),
        ];
        if let Some(range) = query.time_range {
            if let Some((begin, end)) = day_bounds(range) {
                pairs.push(("pubtime_begin_s", begin.to_string()));
                pairs.push(("pubtime_end_s", end.to_string()));
            }
        }
        Ok(
            EngineRequest::get(format!("{base}?{}", query_string(&pairs)))
                .header("Referer", "https://www.bilibili.com/")
                .header("Accept", "application/json, text/javascript, */*; q=0.01")
                .cookie("innersign", "0")
                .cookie("buvid3", BUVID3)
                .cookie("i-wanna-go-back", "-1")
                .cookie("b_ut", "7")
                .cookie("FEED_LIVE_VERSION", "V8")
                .cookie("header_theme_version", "undefined")
                .cookie("home_feed_column", "4"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data.result").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "arcurl");
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(json_str(item, "description"))
                .author(json_str(item, "author"));
            let thumbnail = json_str(item, "pic");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            let published = json_path(item, "pubdate")
                .and_then(serde_json::Value::as_i64)
                .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
                .map(|date| date.to_rfc3339())
                .unwrap_or_default();
            builder = builder.published_date(published);
            let duration = json_str(item, "duration");
            if let Some(seconds) = duration_seconds(&duration) {
                // The API duration is only trusted below 60 minutes.
                if seconds > 0 && seconds <= 3_600 {
                    builder = builder.metadata("length", duration.trim().to_string());
                }
            }
            let aid = json_str(item, "aid");
            if !aid.is_empty() {
                builder = builder.metadata(
                    "iframe_src",
                    format!("{IFRAME_PREFIX}{aid}&high_quality=1&autoplay=false&danmaku=0"),
                );
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE: &str = r##"{"data":{"result":[
      {"title":"<em class=\"keyword\">Rust</em> tutorial","arcurl":"https://www.bilibili.com/video/BV1xx","pic":"https://i0.hdslb.com/a.jpg","description":"A short introduction","author":"Alice","aid":12345,"pubdate":1700000000,"duration":"12:34"},
      {"title":"Conference talk","arcurl":"https://www.bilibili.com/video/BV2yy","pic":"","description":"","author":"Bob","aid":2,"pubdate":1700000000,"duration":"1:02:03"},
      {"title":"No url","arcurl":"","duration":"1:00"}
    ]}}"##;
    fn engine() -> Arc<Bilibili> {
        let config = EngineConfig::new("bilibili", "bilibili", "bil", &["videos"]);
        Bilibili::from_config(&config).unwrap()
    }

    #[test]
    fn parses_video_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.title, "Rust tutorial");
        assert_eq!(result.url, "https://www.bilibili.com/video/BV1xx");
        assert_eq!(result.template, Template::Videos);
        let length = result.metadata.get("length").map(String::as_str);
        assert_eq!(length, Some("12:34"));
        let iframe = result.metadata.get("iframe_src").map(String::as_str);
        assert!(iframe.is_some_and(|value| value.contains("aid=12345")));
        let published = result.published_date.as_deref().unwrap_or_default();
        assert!(published.starts_with("2023-11-14"));
    }

    #[test]
    fn builds_the_paged_request_with_cookies() {
        let mut query = Query::parse("rust async", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("keyword=rust%20async"));
        assert!(request.url.contains("pubtime_begin_s="));
        assert!(request.cookies.iter().any(|(name, _)| name == "buvid3"));
    }
}
