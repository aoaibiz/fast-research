//! 360Search-Videos engine, ported from `searx/engines/360search_videos.py`.

// engine-id: 360search_videos

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
    shortcut: "360search_videos",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://tv.360kan.com";
const COUNT: u32 = 10;

/// 360Search (360 Kan) video search.
pub struct Videoscarch360 {
    meta: EngineMeta,
}

impl Videoscarch360 {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Videoscarch360 {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("count", COUNT.to_string()),
            ("q", query.query_text()),
            ("start", (pageno * COUNT).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/v1/video/list?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "data.result").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            let url = json_str(item, "play_url");
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&json_str(item, "description")))
                .thumbnail(json_str(item, "cover_img"))
                .template(Template::Videos);
            if let Ok(seconds) = json_str(item, "publish_time").parse::<i64>() {
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
    fn parses_video_json() {
        let body = r#"{"data":{"result":[
          {"title":"<i>Clip</i>","play_url":"https://tv.360kan.com/play/1",
           "description":"<p>About a clip.</p>","cover_img":"https://img.example.com/c.jpg",
           "publish_time":"1700000000"}
        ]}}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = Videoscarch360::from_config(&EngineConfig::new(
            "360search videos",
            "360search_videos",
            "360search_videos",
            &["videos"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("clip", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Clip");
                assert_eq!(result.url, "https://tv.360kan.com/play/1");
            }
            _ => panic!("expected a result"),
        }
    }
}
