//! media.ccc.de engine, ported from `searx/engines/ccc_media.py`.

// engine-id: ccc_media

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
    shortcut: "ccc_media",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.media.ccc.de/public/events/search";

/// media.ccc.de video search.
pub struct CccMedia {
    meta: EngineMeta,
}

impl CccMedia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for CccMedia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("page", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{API_URL}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "events").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            let url = json_str(item, "frontend_link");
            if title.is_empty() || url.is_empty() {
                continue;
            }
            // Prefer an mp4 recording (minimal data rates); fall back to the
            // first video recording otherwise.
            let mut iframe_src = String::new();
            if let Some(recordings) = json_path(item, "recordings").and_then(|v| v.as_array()) {
                for rec in recordings {
                    let mime = json_str(rec, "mime_type");
                    if mime.starts_with("video") && (iframe_src.is_empty() || mime == "video/mp4") {
                        iframe_src = json_str(rec, "recording_url");
                    }
                }
            }
            let mut content = json_str(item, "description");
            if content.is_empty() {
                content = json_str(item, "subtitle");
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .thumbnail(json_str(item, "thumb_url"))
                .template(Template::Videos)
                .published_date(json_str(item, "date"));
            let length = json_str(item, "length");
            if !length.is_empty() {
                builder = builder.metadata("length", length);
            }
            if !iframe_src.is_empty() {
                builder = builder.metadata("iframe_src", iframe_src);
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
    fn parses_events_json() {
        let body = r#"{"events":[
          {"title":"Rust in Production","frontend_link":"https://media.ccc.de/v/test-1",
           "description":"A talk about Rust.","thumb_url":"https://media.ccc.de/t.png",
           "date":"2024-06-01T10:00:00Z","length":1800,
           "recordings":[{"mime_type":"video/mp4","recording_url":"https://api.media.ccc.de/r.mp4"},
                         {"mime_type":"video/webm","recording_url":"https://api.media.ccc.de/r.webm"}]}
        ]}"#;
        let response = EngineResponse::new(200, API_URL, body.as_bytes().to_vec());
        let engine = CccMedia::from_config(&EngineConfig::new(
            "media.ccc.de",
            "ccc_media",
            "ccc_media",
            &["videos"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://media.ccc.de/v/test-1");
                assert_eq!(result.title, "Rust in Production");
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://api.media.ccc.de/r.mp4")
                );
            }
            _ => panic!("expected a video result"),
        }
    }
}
