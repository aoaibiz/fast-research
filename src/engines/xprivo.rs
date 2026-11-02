//! XPrivo engine, ported from searx/engines/xprivo.py.
//!
//! The API answers with a Server-Sent Events stream; the first JSON object that
//! carries the results key is parsed, like upstream.
// engine-id: xprivo

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "xpr",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.xprivo.com";
const PAGE_SIZE: u32 = 10;
const SEARCH_TYPES: [&str; 3] = ["all", "news", "videos"];

/// XPrivo.
pub struct Xprivo {
    meta: EngineMeta,
}

impl Xprivo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let search_type = config.opt_str("xprivo_search_type").unwrap_or("all");
        if !SEARCH_TYPES.contains(&search_type) {
            return Err("xprivo: invalid xprivo_search_type".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }

    fn search_type(&self) -> &str {
        self.meta.opt_str("xprivo_search_type").unwrap_or("all")
    }
}

impl Engine for Xprivo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let time_filter = query.time_range.map(|range| range.as_str()).unwrap_or("");
        let body = serde_json::json!({
            "query": query.query_text(),
            "mode": "classic",
            "search_type": self.search_type(),
            "time_filter": time_filter,
            "region": "",
            "offset": (pageno.max(1) - 1) * PAGE_SIZE,
            "source": "",
        });
        let mut request =
            EngineRequest::post(format!("{}/v1/searchengine/completions", self.base_url()))
                .json_body(body)
                .header("Content-Type", "application/json");
        if let Some(key) = self.meta.api_key.as_deref().filter(|key| !key.is_empty()) {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let mut data = None;
        for line in body.lines() {
            let line = line.trim();
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                if value.get("results").is_some() {
                    data = Some(value);
                    break;
                }
            }
        }
        let Some(data) = data else {
            return Ok(Vec::new());
        };
        let Some(entries) = json_path(&data, "results").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let videos = self.search_type() == "videos";
        let mut results = Vec::new();
        for entry in entries {
            let url = json_str(entry, "url");
            let title = json_str(entry, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title).content(json_str(entry, "snippet"));
            if videos {
                builder = builder.template(Template::Videos);
            }
            let thumbnail = json_str(entry, "og_image");
            if is_http_url(&thumbnail) {
                builder = builder.thumbnail(thumbnail);
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
    use crate::engine::Method;

    const FIXTURE: &str = r##"data: {"event":"start"}
data: {"results":[{"url":"https://example.com/a","title":"Alpha","snippet":"First","og_image":"https://img.example/a.png"}]}
data: {"done":true}"##;

    fn engine(search_type: Option<&str>) -> Arc<Xprivo> {
        let mut config = EngineConfig::new("xprivo", "xprivo", "xpr", &["general"]);
        config.api_key = Some("secret".into());
        if let Some(value) = search_type {
            let value = toml::Value::String(value.into());
            config.extra.insert("xprivo_search_type".into(), value);
        }
        Xprivo::from_config(&config).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_sse_stream_and_builds_the_request() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine(None).parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/a");
        assert_eq!(result.content, "First");
        let videos = engine(Some("videos"))
            .parse(&response, &query("rust"))
            .unwrap();
        assert!(
            matches!(&videos[0], EngineItem::Result(video) if video.template == Template::Videos)
        );
        let mut query = query("rust");
        query.time_range = Some(crate::query::TimeRange::Week);
        let request = engine(Some("news")).build_request(&query, 2).unwrap();
        assert_eq!(request.method, Method::Post);
        let body = request.json.expect("json body");
        assert_eq!(body["search_type"], "news");
        assert_eq!(body["offset"].as_u64(), Some(10));
        let authorization = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        assert_eq!(authorization, Some("Bearer secret"));
    }

    #[test]
    fn rejects_unknown_search_types_and_empty_streams() {
        let mut broken = EngineConfig::new("xprivo", "xprivo", "xpr", &["general"]);
        let value = toml::Value::String("maps".into());
        broken.extra.insert("xprivo_search_type".into(), value);
        assert!(Xprivo::from_config(&broken).is_err());
        let response = EngineResponse::new(200, BASE_URL, b"data: {}\n".to_vec());
        assert!(
            engine(None)
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
