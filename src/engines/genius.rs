//! Genius engine, ported from `searx/engines/genius.py`.
// engine-id: genius

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music", "lyrics"],
    shortcut: "gen",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://genius.com/api";
const PAGE_SIZE: i64 = 5;

/// Genius.
pub struct Genius {
    meta: EngineMeta,
}

impl Genius {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Genius {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            (
                "per_page",
                self.meta
                    .opt_i64("page_size")
                    .unwrap_or(PAGE_SIZE)
                    .to_string(),
            ),
        ];
        Ok(
            EngineRequest::get(format!("{API_URL}/search/multi?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(sections) =
            json_path(&json, "response.sections").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for section in sections {
            let Some(hits) = json_path(section, "hits").and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for hit in hits {
                let parsed = match json_str(hit, "type").as_str() {
                    "lyric" | "song" => parse_lyric(hit),
                    "artist" => parse_artist(hit),
                    "album" => parse_album(hit),
                    _ => None,
                };
                if let Some(item) = parsed {
                    results.push(item);
                }
            }
        }
        Ok(results)
    }
}

fn parse_lyric(hit: &serde_json::Value) -> Option<EngineItem> {
    let url = json_str(hit, "result.url");
    let title = json_str(hit, "result.full_title");
    if !is_http_url(&url) || title.is_empty() {
        return None;
    }
    let content = json_path(hit, "highlights")
        .and_then(serde_json::Value::as_array)
        .and_then(|highlights| highlights.first())
        .map(|highlight| json_str(highlight, "value"))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| json_str(hit, "result.title_with_featured"));
    let mut builder = ResultBuilder::new(url, title)
        .content(content)
        .thumbnail(json_str(hit, "result.song_art_image_thumbnail_url"));
    if let Some(timestamp) = json_path(hit, "result.lyrics_updated_at")
        .and_then(serde_json::Value::as_i64)
        .filter(|timestamp| *timestamp != 0)
        .and_then(|timestamp| chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0))
    {
        builder = builder.published_date(timestamp.to_rfc3339());
    }
    let api_path = json_str(hit, "result.api_path");
    if !api_path.is_empty() {
        builder = builder.metadata(
            "iframe_src",
            format!("https://genius.com{api_path}/apple_music_player"),
        );
    }
    Some(builder.build())
}

fn parse_artist(hit: &serde_json::Value) -> Option<EngineItem> {
    let url = json_str(hit, "result.url");
    let title = json_str(hit, "result.name");
    if !is_http_url(&url) || title.is_empty() {
        return None;
    }
    Some(
        ResultBuilder::new(url, title)
            .content("")
            .thumbnail(json_str(hit, "result.image_url"))
            .build(),
    )
}

fn parse_album(hit: &serde_json::Value) -> Option<EngineItem> {
    let url = json_str(hit, "result.url");
    let title = json_str(hit, "result.full_title");
    if !is_http_url(&url) || title.is_empty() {
        return None;
    }
    let mut content = {
        let name_with_artist = json_str(hit, "result.name_with_artist");
        if name_with_artist.is_empty() {
            json_str(hit, "result.name")
        } else {
            name_with_artist
        }
    };
    if let Some(year) =
        json_path(hit, "result.release_date_components.year").and_then(serde_json::Value::as_i64)
    {
        content = format!("{year} / {content}");
    }
    Some(
        ResultBuilder::new(url, title)
            .thumbnail(json_str(hit, "result.cover_art_url"))
            .content(content.trim())
            .build(),
    )
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "response": {
        "sections": [
          {
            "type": "song",
            "hits": [
              {
                "type": "song",
                "highlights": [{"value": "a highlighted line"}],
                "result": {
                  "url": "https://genius.com/Artist-a-song-lyrics",
                  "full_title": "A Song by Artist",
                  "title_with_featured": "A Song",
                  "song_art_image_thumbnail_url": "https://images.genius.com/cover.png",
                  "lyrics_updated_at": 1700000000,
                  "api_path": "/songs/123"
                }
              }
            ]
          }
        ]
      }
    }"#;

    fn engine() -> Arc<Genius> {
        Genius::from_config(&EngineConfig::new(
            "genius",
            "genius",
            "gen",
            &["music", "lyrics"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_song_hits() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("a song", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A Song by Artist");
                assert_eq!(result.url, "https://genius.com/Artist-a-song-lyrics");
                assert_eq!(result.content, "a highlighted line");
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://genius.com/songs/123/apple_music_player")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("a song", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://genius.com/api/search/multi?q=a%20song&page=2&per_page=5"
        );
    }
}
