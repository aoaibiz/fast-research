//! Baidu engine, ported from `searx/engines/baidu.py` (web search).
//!
//! Upstream supports the categories `general`, `images` and `it` through the
//! `baidu_category` setting; this port implements the main `general` variant and
//! reads the category from the `search_type` config key.
// engine-id: baidu

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "bd",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const GENERAL_ENDPOINT: &str = "https://www.baidu.com/s";
const IMAGES_ENDPOINT: &str = "https://image.baidu.com/search/acjson";
const IT_ENDPOINT: &str = "https://kaifa.baidu.com/rest/v1/search";

/// Baidu, the Chinese search engine.
pub struct Baidu {
    meta: EngineMeta,
    categ: String,
    results_per_page: i64,
}

impl Baidu {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config
            .opt_str("search_type")
            .unwrap_or("general")
            .to_string();
        if !matches!(categ.as_str(), "general" | "images" | "it") {
            return Err(format!("baidu: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            results_per_page: config.opt_i64("results_per_page").unwrap_or(10),
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for Baidu {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = ((pageno.max(1) - 1) as i64) * self.results_per_page;
        let mut pairs: Vec<(&str, String)> = match self.categ.as_str() {
            "images" => vec![
                ("word", query.query_text()),
                ("rn", self.results_per_page.to_string()),
                ("pn", offset.to_string()),
                ("tn", "resultjson_com".to_string()),
            ],
            "it" => vec![
                ("wd", query.query_text()),
                ("pageSize", self.results_per_page.to_string()),
                ("pageNum", pageno.max(1).to_string()),
                (
                    "paramList",
                    format!(
                        "page_num={},page_size={}",
                        pageno.max(1),
                        self.results_per_page
                    ),
                ),
                ("position", "0".to_string()),
            ],
            _ => vec![
                ("wd", query.query_text()),
                ("rn", self.results_per_page.to_string()),
                ("pn", offset.to_string()),
                ("tn", "json".to_string()),
            ],
        };
        if let Some(seconds) = TimeRange::seconds(query.time_range) {
            let now = chrono::Utc::now().timestamp();
            let past = now - seconds;
            match self.categ.as_str() {
                "general" => pairs.push(("gpc", format!("stf={past},{now}|stftype=1"))),
                "it" => {
                    if let Some((_, value)) = pairs.iter_mut().find(|(key, _)| *key == "paramList")
                    {
                        value.push_str(&format!(",timestamp_range={past}-{now}"));
                    }
                }
                _ => {}
            }
        }
        let endpoint = match self.categ.as_str() {
            "images" => IMAGES_ENDPOINT,
            "it" => IT_ENDPOINT,
            _ => GENERAL_ENDPOINT,
        };
        Ok(EngineRequest::get(format!(
            "{endpoint}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // Baidu redirects bots to wappass.baidu.com for a captcha.
        if response
            .header("location")
            .is_some_and(|location| location.contains("wappass.baidu.com/static/captcha"))
        {
            return Err(EngineError::Captcha);
        }
        let body = match self.categ.as_str() {
            // Baidu's JSON encoder escapes slashes and single quotes.
            "images" => response.text().replace(r"\/", "/").replace(r"\'", "'"),
            _ => response.text(),
        };
        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|error| EngineError::Parse(format!("invalid JSON response: {error}")))?;
        if json_path(&json, "antiFlag").and_then(serde_json::Value::as_i64) == Some(1) {
            return Err(EngineError::AccessDenied);
        }
        let mut results = Vec::new();
        if self.categ == "it" {
            let Some(items) =
                json_path(&json, "data.documents.data").and_then(serde_json::Value::as_array)
            else {
                return Ok(results);
            };
            for item in items {
                let title = json_str(item, "techDocDigest.title");
                let url = json_str(item, "techDocDigest.url");
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(url, decode_entities(&title))
                        .content(decode_entities(&json_str(item, "techDocDigest.summary")))
                        .build(),
                );
            }
            return Ok(results);
        }
        let Some(items) = json_path(&json, "feed.entry").and_then(serde_json::Value::as_array)
        else {
            return Ok(results);
        };
        for item in items {
            let title = json_str(item, "title");
            let url = json_str(item, "url");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, decode_entities(&title))
                .content(decode_entities(&json_str(item, "abs")));
            if let Some(timestamp) = json_path(item, "time").and_then(serde_json::Value::as_i64) {
                if let Some(instant) = chrono::DateTime::from_timestamp(timestamp, 0) {
                    builder = builder.published_date(instant.format("%Y-%m-%d").to_string());
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "feed": {
        "entry": [
          {"title": "Rust &amp; Cargo", "url": "https://www.rust-lang.org/",
           "abs": "Official site &gt; here", "time": 1700000000}
        ]
      }
    }"#;

    fn engine() -> Arc<Baidu> {
        Baidu::from_config(&EngineConfig::new("baidu", "baidu", "bd", &["general"])).unwrap()
    }

    #[test]
    fn parses_general_results() {
        let response =
            EngineResponse::new(200, "https://www.baidu.com/s", FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust & Cargo");
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.content, "Official site > here");
                assert!(result.published_date.is_some());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_paged_general_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with("https://www.baidu.com/s?"));
        assert!(request.url.contains("rn=10"));
        assert!(request.url.contains("pn=20"));
        assert!(request.url.contains("tn=json"));
    }

    #[test]
    fn detects_the_captcha_redirect() {
        let mut response = EngineResponse::new(302, "https://www.baidu.com/s", b"".to_vec());
        response.headers.insert(
            "location".into(),
            "https://wappass.baidu.com/static/captcha".into(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }
}
