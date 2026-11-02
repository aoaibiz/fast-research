//! MediathekViewWeb engine, ported from searx/engines/mediathekviewweb.py.

// engine-id: mediathekviewweb

use std::sync::Arc;

use serde_json::json;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "mvw",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://mediathekviewweb.de";
const PAGE_SIZE: u32 = 10;

/// MediathekViewWeb, the German public broadcast archive.
pub struct Mediathekviewweb {
    meta: EngineMeta,
}

impl Mediathekviewweb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The duration in the H:MM:SS form Python's timedelta prints.
fn hms(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours}:{minutes:02}:{seconds:02}")
}

impl Engine for Mediathekviewweb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = json!({
            "queries": [{
                "fields": ["title", "topic", "description"],
                "query": query.query_text(),
            }],
            "sortBy": "timestamp",
            "sortOrder": "desc",
            "future": true,
            "offset": pageno.saturating_sub(1) * PAGE_SIZE,
            "size": PAGE_SIZE,
        });
        Ok(
            EngineRequest::post(format!("{}/api/query", base_url(&self.meta)))
                .header("Content-type", "text/plain")
                .json_body(body),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "result.results").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let video_url = ["url_video_hd", "url_video", "url_video_low"]
                .iter()
                .map(|key| json_str(item, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            if !is_http(&video_url) {
                continue;
            }
            let title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            let channel = json_str(item, "channel");
            let duration = json_path(item, "duration")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let length = hms(duration);
            results.push(
                ResultBuilder::new(video_url.clone(), format!("{channel}: {title} ({length})"))
                    .content(json_str(item, "description"))
                    .template(Template::Videos)
                    .metadata("length", length)
                    .metadata("iframe_src", video_url)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "result": {
        "results": [
          {"channel": "ARD", "title": "Tagesschau", "topic": "news",
           "description": "Die Nachrichten", "duration": 900,
           "url_video_hd": "https://example.com/hd.mp4", "url_video": "", "url_video_low": ""},
          {"channel": "ZDF", "title": "Ohne Video", "description": "x", "duration": 60,
           "url_video_hd": "", "url_video": "", "url_video_low": ""}
        ]
      }
    }"##;

    fn engine() -> Arc<Mediathekviewweb> {
        Mediathekviewweb::from_config(&EngineConfig::new(
            "mediathekviewweb",
            "mediathekviewweb",
            "mvw",
            &["videos"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_post_body() {
        let request = engine().build_request(&query("tagesschau"), 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://mediathekviewweb.de/api/query");
        let body = request.json.as_ref().unwrap();
        assert_eq!(body["queries"][0]["query"], "tagesschau");
        assert_eq!(body["queries"][0]["fields"][0], "title");
        assert_eq!(body["offset"].as_u64(), Some(10));
        assert_eq!(body["size"].as_u64(), Some(10));
    }

    #[test]
    fn parses_video_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("tagesschau")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "ARD: Tagesschau (0:15:00)");
                assert_eq!(result.url, "https://example.com/hd.mp4");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "Die Nachrichten");
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("0:15:00")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://example.com/hd.mp4")
                );
            }
            _ => panic!("expected a video result"),
        }
    }

    #[test]
    fn missing_result_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine().parse(&response, &query("tagesschau")).unwrap();
        assert!(items.is_empty());
    }
}
