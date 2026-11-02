//! Dailymotion engine, ported from `searx/engines/dailymotion.py`.

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
    shortcut: "dm",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://api.dailymotion.com/videos";
const PAGE_SIZE: u32 = 10;
const FIELDS: &str = "id,title,url,thumbnail_240_url,duration,owner.screenname,created_time";
const IFRAME_SRC: &str = "https://www.dailymotion.com/embed/video/{video_id}";

/// The family filter is on unless the user asked for safe search off.
fn family_filter(safesearch: u8) -> &'static str {
    if safesearch == 0 { "false" } else { "true" }
}

/// Upstream's time_delta_dict, in days.
fn time_delta_days(range: TimeRange) -> u64 {
    match range {
        TimeRange::Day => 1,
        TimeRange::Week => 7,
        TimeRange::Month => 31,
        TimeRange::Year => 365,
    }
}

/// Dailymotion's public videos API.
pub struct Dailymotion {
    meta: EngineMeta,
}

impl Dailymotion {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Dailymotion {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("search", query.query_text()),
            ("fields", FIELDS.to_string()),
            ("limit", PAGE_SIZE.to_string()),
            ("page", pageno.max(1).to_string()),
            ("family_filter", family_filter(query.safesearch).to_string()),
            ("thumbnail_ratio", "original".to_string()),
            ("sort", "relevance".to_string()),
        ];
        let language = locale_language(&query.locale);
        if language != "all" {
            pairs.push(("languages", language));
        }
        if let Some(range) = query.time_range {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let created_after = now.saturating_sub(time_delta_days(range) * 86_400);
            pairs.push(("created_after", created_after.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(message) = json_path(&json, "error.message").and_then(|value| value.as_str()) {
            return Err(EngineError::Message(message.to_string()));
        }
        let Some(items) = json_path(&json, "list").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            let url = json_str(item, "url");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let author = json_path(item, "owner")
                .and_then(|owner| owner.get("screenname"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let thumbnail = json_str(item, "thumbnail_240_url").replace("http://", "https://");
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .author(author)
                .thumbnail(thumbnail)
                .published_date(published_date(item))
                .metadata("length", length(item))
                .metadata("duration", json_str(item, "duration"));
            let video_id = json_str(item, "id");
            if !video_id.is_empty()
                && json_path(item, "allow_embed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            {
                builder =
                    builder.metadata("iframe_src", IFRAME_SRC.replace("{video_id}", &video_id));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The creation time as an RFC 3339 string.
fn published_date(item: &serde_json::Value) -> String {
    let seconds = json_path(item, "created_time").and_then(serde_json::Value::as_i64);
    match seconds.and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)) {
        Some(date) => date.to_rfc3339(),
        None => String::new(),
    }
}

/// The duration formatted as MM:SS or HH:MM:SS.
fn length(item: &serde_json::Value) -> String {
    let Some(seconds) = json_path(item, "duration").and_then(serde_json::Value::as_u64) else {
        return String::new();
    };
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "page": 1,
      "limit": 10,
      "explicit": false,
      "list": [
        {
          "id": "x8abc",
          "title": "Rust in 100 seconds",
          "url": "https://www.dailymotion.com/video/x8abc",
          "thumbnail_240_url": "http://s1.dmcdn.net/x8abc.jpg",
          "duration": 125,
          "created_time": 1700000000,
          "allow_embed": true,
          "owner": {"id": "u1", "screenname": "Fireship"}
        }
      ]
    }"#;

    fn engine() -> Arc<Dailymotion> {
        Dailymotion::from_config(&EngineConfig::new(
            "dailymotion",
            "dailymotion",
            "dm",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_video_list() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust in 100 seconds");
                assert_eq!(result.url, "https://www.dailymotion.com/video/x8abc");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.author.as_deref(), Some("Fireship"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://s1.dmcdn.net/x8abc.jpg")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("02:05")
                );
                let expected = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
                    .unwrap()
                    .to_rfc3339();
                assert_eq!(result.published_date.as_deref(), Some(expected.as_str()));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn api_errors_become_engine_errors() {
        let body = r#"{"error": {"code": 400, "message": "invalid parameter"}}"#;
        let response = EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Message(_))
        ));
    }

    #[test]
    fn forwards_paging_safesearch_and_time_range() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        query.safesearch = 0;
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.contains("page=3"));
        assert!(request.url.contains("limit=10"));
        assert!(request.url.contains("family_filter=false"));
        assert!(request.url.contains("created_after="));
    }
}
