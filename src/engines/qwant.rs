//! Qwant engine, ported from `searx/engines/qwant.py`.
//!
//! Qwant serves web, news, images and videos from a single undocumented API;
//! the variant is selected with the `qwant_categ` option (one of `web`,
//! `news`, `images`, `videos`).  The `datadome` cookie cache of upstream is
//! not ported: it only exists to survive Qwant's bot protection and the
//! framework does not expose a per engine key/value store here.
// engine-id: qwant

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "qw",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.qwant.com";
const API_URL: &str = "https://api.qwant.com/v3/search/";

/// Qwant (web, news, images or videos, depending on `qwant_categ`).
pub struct Qwant {
    meta: EngineMeta,
}

impl Qwant {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let engine = Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        };
        match engine.category() {
            "web" | "news" | "images" | "videos" => Ok(Arc::new(engine)),
            other => Err(format!(
                "qwant: qwant_categ must be web, news, images or videos (got {other:?})"
            )),
        }
    }

    /// The configured variant, upstream's `qwant_categ`.
    fn category(&self) -> &str {
        self.meta.opt_str("qwant_categ").unwrap_or("web")
    }

    fn api_url(&self) -> String {
        self.meta
            .opt_str("api_url")
            .unwrap_or(API_URL)
            .trim_end_matches('/')
            .to_string()
    }

    fn site_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    /// Upstream's `traits.get_region(searxng_locale, default="en_US")`.
    ///
    /// The engine traits table is not available, so the region is derived from
    /// the locale: `ja-JP` becomes `ja_JP`, a language only locale `en`
    /// becomes `en_US`.
    fn qwant_locale(query: &Query) -> String {
        let language = locale_language(&query.locale);
        if language == "all" {
            return "en_US".to_string();
        }
        let region = locale_region(&query.locale);
        if region == "all" {
            format!("{language}_US")
        } else {
            format!("{language}_{}", region.to_uppercase())
        }
    }
}

impl Engine for Qwant {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let category = self.category();
        if query.query_text().is_empty() {
            return Err(EngineError::Message("qwant: empty query".into()));
        }
        let results_per_page: u32 = if category == "images" { 50 } else { 10 };
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("count", results_per_page.to_string()),
            ("locale", Self::qwant_locale(query)),
            (
                "offset",
                (pageno.max(1) - 1)
                    .saturating_mul(results_per_page)
                    .to_string(),
            ),
            ("tgp", "1".to_string()),
            ("device", "desktop".to_string()),
            ("safesearch", query.safesearch.min(2).to_string()),
            ("displayed", "true".to_string()),
            ("llm", "true".to_string()),
        ];
        let site = self.site_url();
        Ok(EngineRequest::get(format!(
            "{}/{category}?{}",
            self.api_url(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json")
        .header("Referer", &format!("{site}/"))
        .header("Origin", &site))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let status = json_str(&json, "status");
        if status != "success" {
            let data = json_path(&json, "data")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let error_code = json_path(&data, "error_code").and_then(serde_json::Value::as_i64);
            if error_code == Some(24) {
                return Err(EngineError::TooManyRequests);
            }
            if json.get("url").is_some() {
                return Err(EngineError::Captcha);
            }
            if response.status == 403 {
                return Err(EngineError::AccessDenied);
            }
            let message = json_path(&data, "message")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let error_code = error_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return Err(EngineError::Message(format!("{message} ({error_code})")));
        }

        let category = self.category();
        let data = json_path(&json, "data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mainline: Vec<serde_json::Value> = if category == "web" {
            json_path(&data, "result.items.mainline")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        } else {
            let items = json_path(&data, "result.items")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            vec![serde_json::json!({ "type": category, "items": items })]
        };

        let mut results = Vec::new();
        for row in &mainline {
            let row_type = json_str(row, "type");
            if row_type != category || row_type == "ads" {
                continue;
            }
            let Some(items) = json_path(row, "items").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for item in items {
                if let Some(result) = parse_item(category, item) {
                    results.push(result);
                }
            }
        }
        Ok(results)
    }
}

fn parse_item(category: &str, item: &serde_json::Value) -> Option<EngineItem> {
    let title = json_str(item, "title");
    let url = json_str(item, "url");
    if !is_http_url(&url) || title.is_empty() {
        return None;
    }
    let published = qwant_date(item);
    match category {
        "images" => {
            // Upstream passes `item["thumbnail"] or ""` and `item["media"] or ""`
            // straight through, without a published date.
            let resolution = format!("{} x {}", json_str(item, "width"), json_str(item, "height"));
            Some(
                ResultBuilder::new(url, title)
                    .image(json_str(item, "media"))
                    .thumbnail(json_str(item, "thumbnail"))
                    .template(Template::Images)
                    .metadata("resolution", resolution)
                    .metadata("img_format", json_str(item, "thumb_type"))
                    .build(),
            )
        }
        "news" => {
            let thumbnail = json_path(item, "media")
                .and_then(serde_json::Value::as_array)
                .and_then(|media| media.first())
                .map(|media| json_str(media, "pict.url"))
                .unwrap_or_default();
            Some(
                ResultBuilder::new(url, title)
                    .content(json_str(item, "desc"))
                    .thumbnail(thumbnail)
                    .published_date(published)
                    .build(),
            )
        }
        "videos" => {
            let mut parts = Vec::new();
            let description = json_str(item, "desc");
            if !description.is_empty() {
                parts.push(description);
            }
            let source = json_str(item, "source");
            if !source.is_empty() {
                parts.push(format!("Source: {source} "));
            }
            let channel = json_str(item, "channel");
            if !channel.is_empty() {
                parts.push(format!("Channel: {channel} "));
            }
            let thumbnail = json_str(item, "thumbnail").replacen(
                "https://s2.qwant.com",
                "https://s1.qwant.com",
                1,
            );
            let duration = json_path(item, "duration")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            Some(
                ResultBuilder::new(url, title)
                    .content(parts.join(" // "))
                    .thumbnail(thumbnail)
                    .published_date(published)
                    .template(Template::Videos)
                    .metadata("length", format_duration(duration / 1000))
                    .build(),
            )
        }
        // "web": upstream adds no published date either.
        _ => Some(
            ResultBuilder::new(url, title)
                .content(json_str(item, "desc"))
                .build(),
        ),
    }
}

/// Unix seconds (or milliseconds for news) as an RFC 3339 timestamp.
fn qwant_date(item: &serde_json::Value) -> String {
    let Some(raw) = json_path(item, "date").and_then(serde_json::Value::as_f64) else {
        return String::new();
    };
    if raw <= 0.0 {
        return String::new();
    }
    // Upstream retries with milliseconds when the seconds value overflows.
    let seconds = if raw > 253_402_300_799.0 {
        raw / 1000.0
    } else {
        raw
    };
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds as i64, 0)
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

fn format_duration(seconds: i64) -> String {
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

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "status": "success",
      "data": {
        "result": {
          "items": {
            "mainline": [
              {
                "type": "web",
                "items": [
                  {
                    "title": "Rust Programming Language",
                    "url": "https://www.rust-lang.org/",
                    "desc": "A language empowering everyone."
                  }
                ]
              }
            ]
          }
        }
      }
    }"#;

    fn engine() -> Arc<Qwant> {
        Qwant::from_config(&EngineConfig::new(
            "qwant",
            "qwant",
            "qw",
            &["general", "web"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_web_mainline() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Programming Language");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "A language empowering everyone.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_web_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.qwant.com/v3/search/web?")
        );
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("count=10"));
        assert!(request.url.contains("offset=10"));
        assert!(request.url.contains("locale=en_US"));
    }

    #[test]
    fn api_errors_are_classified() {
        let body = r#"{"status": "error", "data": {"error_code": 24, "message": ["too many"]}}"#;
        let response = EngineResponse::new(200, API_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::TooManyRequests)
        ));
    }
}
