//! Sogou-Images engine, ported from searx/engines/sogou_images.py.
// engine-id: sogou_images

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "sogoui",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pic.sogou.com";
const STATE_RE: &str = "(?s)window\\.__INITIAL_STATE__\\s*=\\s*(\\{.*?\\});";

/// Sogou image search.
pub struct SogouImages {
    meta: EngineMeta,
}

impl SogouImages {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for SogouImages {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let start = pageno.saturating_sub(1) * 48;
        let pairs = vec![("query", query.query_text()), ("start", start.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/pics?{}",
            base.trim_end_matches('/'),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let pattern = regex::Regex::new(STATE_RE)
            .map_err(|error| EngineError::Parse(format!("invalid state pattern: {error}")))?;
        let Some(captures) = pattern.captures(&body) else {
            return Ok(Vec::new());
        };
        let Some(payload) = captures.get(1) else {
            return Ok(Vec::new());
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(payload.as_str()) else {
            return Ok(Vec::new());
        };
        let Some(items) =
            json_path(&json, "searchList.searchList").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "title");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let picture = json_str(item, "picUrl");
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "content_major"))
                .template(Template::Images)
                .metadata("source", json_str(item, "ch_site_name"));
            if !picture.is_empty() {
                builder = builder.thumbnail(picture.clone()).image(picture);
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
    fn engine() -> Arc<SogouImages> {
        SogouImages::from_config(&EngineConfig::new(
            "sogou images",
            "sogou_images",
            "sogoui",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_initial_state() {
        let body = r##"<html><script>window.__INITIAL_STATE__ = {"searchList":{"searchList":[{"url":"https://example.com/photo","picUrl":"https://img.example.com/cat.jpg","title":"Cat photo","content_major":"A cat","ch_site_name":"example.com"}]}};</script></html>"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/photo");
                assert_eq!(result.title, "Cat photo");
                assert_eq!(result.content, "A cat");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/cat.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.example.com/cat.jpg")
                );
                assert_eq!(
                    result.metadata.get("source").map(String::as_str),
                    Some("example.com")
                );
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn missing_state_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://pic.sogou.com/pics?query=cat&start=48");
    }
}
