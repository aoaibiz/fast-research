//! ChinaSo engine, ported from `searx/engines/chinaso.py`.

// engine-id: chinaso

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    // Upstream declares no categories; the engine is normally configured for
    // news search, so it stays out of any tab by default.
    categories: &["general"],
    shortcut: "chinaso",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.chinaso.com";
const RESULTS_PER_PAGE: u32 = 10;

// Upstream sends a random base64 uid tracking cookie; without a random source
// in the framework we send a fixed cookie and note the deviation.
const UID_COOKIE: &str = "UXVO4MRuHJi2RpRcO3c8Ag==";

/// ChinaSo search.
pub struct ChinaSo {
    meta: EngineMeta,
}

impl ChinaSo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for ChinaSo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("pn", pageno.to_string()),
            ("ps", RESULTS_PER_PAGE.to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "24h",
                TimeRange::Week => "1w",
                TimeRange::Month => "1m",
                TimeRange::Year => "1y",
            };
            pairs.push(("stime", value.to_string()));
            pairs.push(("etime", "now".to_string()));
        }
        let source = self.meta.opt_str("chinaso_news_source").unwrap_or("all");
        if source != "all" {
            if source == "EPAPER" {
                pairs.push(("type", "EPAPER".to_string()));
            } else {
                pairs.push(("cate", source.to_string()));
            }
        }
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/v5/general/v1/web/search?{}",
            query_string(&pairs)
        ))
        .cookie("uid", UID_COOKIE))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        // A structurally broken body is a parse error, an empty data object is
        // an empty result list (upstream status 0 / Sorry about that).
        let data = match json.get("data") {
            Some(value) if !value.is_null() => value,
            _ => return Err(EngineError::Parse("missing data object".into())),
        };
        let entries = match data.get("data") {
            Some(value) if !value.is_null() => match value.as_array() {
                Some(entries) => entries,
                None => return Err(EngineError::Parse("invalid data list".into())),
            },
            _ => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for entry in entries {
            let title = strip_tags(&json_str(entry, "title"));
            let url = json_str(entry, "url");
            if title.is_empty() || url.is_empty() {
                continue;
            }
            if !url.starts_with("http://") && !url.starts_with("https://") {
                continue;
            }
            let mut builder =
                ResultBuilder::new(url, title).content(strip_tags(&json_str(entry, "snippet")));
            let timestamp = json_str(entry, "timestamp");
            if let Ok(seconds) = timestamp.parse::<i64>() {
                builder = builder.published_date(seconds.to_string());
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_news_results() {
        let body = r#"{"data":{"data":[
          {"title":"<b>News</b> item","url":"https://news.example.com/1","snippet":"Snippet","timestamp":"1700000000"}
        ]}}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = ChinaSo::from_config(&EngineConfig::new(
            "chinaso",
            "chinaso",
            "chinaso",
            &["general"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("news", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "News item");
                assert_eq!(result.url, "https://news.example.com/1");
            }
            _ => panic!("expected a result"),
        }
    }
}
