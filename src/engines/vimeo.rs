//! Vimeo engine, ported from searx/engines/vimeo.py.
//!
//! Upstream fetches a JWT from https://vimeo.com/_next/jwt before every
//! search. The synchronous request builder cannot perform that fetch, so the
//! token is read from the engine configuration instead: the jwt option first,
//! then api_key. Without a configured token the request is sent without an
//! authorization header.
// engine-id: vimeo

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "vm",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.vimeo.com";
const RESULTS_PER_PAGE: u32 = 20;
const ACCEPT: &str = "application/vnd.vimeo.*+json;version=3.3";

/// Vimeo's clip search API.
pub struct Vimeo {
    meta: EngineMeta,
}

impl Vimeo {
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

    fn token(&self) -> Option<&str> {
        self.meta
            .opt_str("jwt")
            .or(self.meta.api_key.as_deref())
            .map(str::trim)
            .filter(|token| !token.is_empty())
    }
}

impl Engine for Vimeo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("filter_type", "clip".to_string()),
            ("query", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("per_page", RESULTS_PER_PAGE.to_string()),
        ];
        let mut request = EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&pairs)
        ))
        .header("content-type", "application/json")
        .header("Accept", ACCEPT);
        if let Some(token) = self.token() {
            request = request.header("authorization", &format!("jwt {token}"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let Some(clip) = item.get("clip") else {
                continue;
            };
            let title = json_str(clip, "name");
            let url = json_str(clip, "link");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url.clone(), title)
                .template(Template::Videos)
                .author(json_str(clip, "user.name"))
                .thumbnail(json_str(clip, "pictures.base_link"))
                .published_date(iso_date(&json_str(clip, "created_time")))
                .metadata("length", duration(json_path(clip, "duration")));
            let video_id = video_id(&url);
            if !video_id.is_empty() {
                let iframe = format!("https://player.vimeo.com/video/{video_id}");
                builder = builder.metadata("iframe_src", iframe);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The clip id is the last path segment of the clip link.
fn video_id(url: &str) -> String {
    url::Url::parse(url)
        .map(|url| url.path().trim_matches('/').to_string())
        .unwrap_or_default()
}

/// Normalise the created_time of the API to RFC 3339.
fn iso_date(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

/// The duration formatted as MM:SS or HH:MM:SS.
fn duration(value: Option<&serde_json::Value>) -> String {
    let Some(seconds) = value.and_then(serde_json::Value::as_u64) else {
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

    const FIXTURE: &str = r##"{
      "total": 1,
      "page": 1,
      "per_page": 20,
      "data": [
        {
          "clip": {
            "name": "Rust in 100 seconds",
            "link": "https://vimeo.com/123456",
            "duration": 125,
            "created_time": "2024-05-01T12:00:00+00:00",
            "user": {"name": "Fireship"},
            "pictures": {"base_link": "https://i.vimeocdn.com/video/123.jpg"}
          }
        }
      ]
    }"##;

    fn engine() -> Arc<Vimeo> {
        Vimeo::from_config(&EngineConfig::new("vimeo", "vimeo", "vm", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_clip_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust in 100 seconds");
                assert_eq!(result.url, "https://vimeo.com/123456");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.author.as_deref(), Some("Fireship"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.vimeocdn.com/video/123.jpg")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01T12:00:00+00:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("02:05")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://player.vimeo.com/video/123456")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn request_carries_paging_and_the_jwt() {
        let mut config = EngineConfig::new("vimeo", "vimeo", "vm", &["videos"]);
        config.api_key = Some("token123".to_string());
        let request = Vimeo::from_config(&config)
            .unwrap()
            .build_request(&query(), 2)
            .unwrap();
        assert_eq!(
            request.url,
            "https://api.vimeo.com/search?filter_type=clip&query=rust&page=2&per_page=20"
        );
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "jwt token123"
        );
        assert_eq!(
            request.headers.get("Accept").unwrap().to_str().unwrap(),
            ACCEPT
        );
    }

    #[test]
    fn a_missing_container_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"{\"total\": 0}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
