//! Yandex Music engine, ported from searx/engines/yandex_music.py.
//!
//! The search handler returns JSON; only tracks are mapped to results.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: yandex_music

const BASE_URL: &str = "https://music.yandex.ru";

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// Yandex Music track search.
pub struct YandexMusic {
    meta: EngineMeta,
}

impl YandexMusic {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for YandexMusic {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        Ok(EngineRequest::get(format!(
            "{base}/handlers/music-search.jsx?{}",
            query_string(&[
                ("text", query.query_text().to_string()),
                ("page", pageno.saturating_sub(1).to_string()),
            ])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        let Some(tracks) = json_path(&json, "tracks.items").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for track in tracks {
            if json_str(track, "type") != "music" {
                continue;
            }
            let track_id = json_str(track, "id");
            let album_id = json_str(track, "albums.0.id");
            let album_title = json_str(track, "albums.0.title");
            let artist = json_str(track, "artists.0.name");
            let title = json_str(track, "title");
            if track_id.is_empty() || album_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{base}/album/{album_id}/track/{track_id}");
            let content = format!("[{album_title}] {artist} - {title}");
            results.push(
                ResultBuilder::new(url, title)
                    .content(content)
                    .template(Template::Music)
                    .metadata(
                        "iframe_src",
                        format!("{base}/iframe/track/{track_id}/{album_id}"),
                    )
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<YandexMusic> {
        YandexMusic::from_config(&EngineConfig::new(
            "yandex_music",
            "yandex_music",
            "",
            &["music"],
        ))
        .expect("engine")
    }

    #[test]
    fn builds_the_search_url() {
        let query = Query::parse("muse", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://music.yandex.ru/handlers/music-search.jsx?text=muse&page=1"
        );
    }

    #[test]
    fn parses_the_tracks() {
        let body = r##"{"tracks":{"items":[
            {"type":"music","id":"42","title":"Song","albums":[{"id":"7","title":"Album"}],
             "artists":[{"name":"Artist"}]},
            {"type":"podcast","id":"1","title":"Ignored","albums":[{"id":"2"}],"artists":[{"name":"X"}]}
        ]}}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("song", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://music.yandex.ru/album/7/track/42");
                assert_eq!(result.title, "Song");
                assert_eq!(result.content, "[Album] Artist - Song");
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://music.yandex.ru/iframe/track/42/7")
                );
            }
            _ => panic!("expected a result"),
        }
    }
}
