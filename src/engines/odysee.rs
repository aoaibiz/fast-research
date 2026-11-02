//! Odysee engine, ported from searx/engines/odysee.py.
// engine-id: odysee

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
    shortcut: "od",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://lighthouse.odysee.tv/search";
const RESULTS_PER_PAGE: u32 = 20;
const INCLUDE: &str = "channel,thumbnail_url,title,description,duration,release_time";

/// The Odysee lighthouse search API.
pub struct Odysee {
    meta: EngineMeta,
}

impl Odysee {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn endpoint(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
    }
}

impl Engine for Odysee {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("s", query.query_text()),
            ("size", RESULTS_PER_PAGE.to_string()),
            ("from", ((pageno.max(1) - 1) * RESULTS_PER_PAGE).to_string()),
            ("include", INCLUDE.to_string()),
            ("mediaType", "video".to_string()),
        ];
        let language = locale_language(&query.locale);
        if language != "all" {
            pairs.push(("language", language));
        }
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "today",
                TimeRange::Week => "thisweek",
                TimeRange::Month => "thismonth",
                TimeRange::Year => "thisyear",
            };
            pairs.push(("time_filter", value.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{}?{}", self.endpoint(), query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let name = json_str(item, "name");
            let claim_id = json_str(item, "claimId");
            let title = json_str(item, "title");
            if name.is_empty() || claim_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("https://odysee.com/{name}:{claim_id}");
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Videos)
                .content(json_str(item, "description"))
                .author(json_str(item, "channel"))
                .published_date(release_date(&json_str(item, "release_time")))
                .metadata("length", duration(item))
                .metadata(
                    "iframe_src",
                    format!("https://odysee.com/$/embed/{name}:{claim_id}"),
                );
            let thumbnail = json_str(item, "thumbnail_url");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(format!(
                    "https://thumbnails.odycdn.com/optimize/s:390:0/quality:85/plain/{thumbnail}"
                ));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream keeps only the date part of release_time.
fn release_date(value: &str) -> String {
    let date = value.split('T').next().unwrap_or_default();
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%dT00:00:00").to_string())
        .unwrap_or_default()
}

/// The duration in seconds formatted as MM:SS or HH:MM:SS.
fn duration(item: &serde_json::Value) -> String {
    let Some(seconds) = json_path(item, "duration").and_then(serde_json::Value::as_i64) else {
        return String::new();
    };
    let seconds = seconds.max(0);
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

    const FIXTURE: &str = r##"[
      {
        "name": "some-video",
        "claimId": "abc123",
        "title": "Rust on Odysee",
        "description": null,
        "channel": "Rust Channel",
        "thumbnail_url": "https://thumbs.odycdn.com/abc.jpg",
        "duration": 425,
        "release_time": "2024-05-01T12:00:00Z"
      },
      {
        "name": "empty",
        "claimId": "def456",
        "title": "",
        "duration": 10,
        "release_time": "2024-01-01T00:00:00Z"
      }
    ]"##;

    fn engine() -> Arc<Odysee> {
        Odysee::from_config(&EngineConfig::new("odysee", "odysee", "od", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_search_response() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust on Odysee");
                assert_eq!(result.url, "https://odysee.com/some-video:abc123");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.content, "");
                assert_eq!(result.author.as_deref(), Some("Rust Channel"));
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T00:00:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("07:05")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://odysee.com/$/embed/some-video:abc123")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some(
                        "https://thumbnails.odycdn.com/optimize/s:390:0/quality:85/plain/https://thumbs.odycdn.com/abc.jpg"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn forwards_paging_language_and_time_range() {
        let mut query = query();
        query.locale = "ja-JP".to_string();
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://lighthouse.odysee.tv/search?")
        );
        assert!(request.url.contains("s=rust"));
        assert!(request.url.contains("size=20"));
        assert!(request.url.contains("from=20"));
        assert!(request.url.contains("mediaType=video"));
        assert!(request.url.contains("language=ja"));
        assert!(request.url.contains("time_filter=thisweek"));
    }

    #[test]
    fn a_non_array_body_is_not_a_parse_error() {
        let response = EngineResponse::new(200, BASE_URL, b"{\"error\": \"nope\"}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
