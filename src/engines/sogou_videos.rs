//! Sogou-Videos engine, ported from searx/engines/sogou_videos.py.
// engine-id: sogou_videos

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
    shortcut: "sogouv",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://v.sogou.com";

/// Sogou video search.
pub struct SogouVideos {
    meta: EngineMeta,
}

impl SogouVideos {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for SogouVideos {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let pairs = vec![
            ("page", pageno.to_string()),
            ("pagesize", "10".to_string()),
            ("query", query.query_text()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/api/video/shortVideoV2?{}",
            base.trim_end_matches('/'),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data.list").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "titleEsc");
            let url = json_str(item, "url");
            let url = if url.starts_with("/vc/np") {
                format!("{base}{url}")
            } else {
                url
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let date = json_str(item, "date");
            let duration = json_str(item, "duration");
            // Upstream only fills date and length when both fields are present.
            let published = if date.is_empty() || duration.is_empty() {
                String::new()
            } else {
                date
            };
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "site"))
                .template(Template::Videos)
                .metadata("length", format_length(&duration))
                .published_date(published);
            let thumbnail = json_str(item, "picurl");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// MM:SS, zero padded; anything else is dropped like upstream's strptime.
fn format_length(duration: &str) -> String {
    let parts: Vec<&str> = duration.split(':').collect();
    if parts.len() != 2 {
        return String::new();
    }
    match (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
        (Ok(minutes), Ok(seconds)) => format!("{minutes:02}:{seconds:02}"),
        _ => String::new(),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> Arc<SogouVideos> {
        SogouVideos::from_config(&EngineConfig::new(
            "sogou videos",
            "sogou_videos",
            "sogouv",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_video_list() {
        let body = r##"{"data":{"list":[{"titleEsc":"Alpha video","url":"/vc/np/abc","site":"example.com","duration":"04:10","date":"2024-05-06T10:00:00","picurl":"http://img.example.com/v.jpg"},{"titleEsc":"","url":"https://example.com/skip"}]}}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://v.sogou.com/vc/np/abc");
                assert_eq!(result.title, "Alpha video");
                assert_eq!(result.content, "example.com");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-06T10:00:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("04:10")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("http://img.example.com/v.jpg")
                );
            }
            _ => panic!("expected a video result"),
        }
    }

    #[test]
    fn missing_list_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, "{\"data\":{}}".as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://v.sogou.com/api/video/shortVideoV2?")
        );
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("pagesize=10"));
        assert!(request.url.contains("query=cat"));
    }
}
