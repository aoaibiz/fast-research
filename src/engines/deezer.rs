//! Deezer engine, ported from `searx/engines/deezer.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music"],
    shortcut: "dz",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://api.deezer.com/search";
/// Upstream requests 25 tracks per page.
const PAGE_SIZE: u32 = 25;

/// Deezer's public search API.
pub struct Deezer {
    meta: EngineMeta,
}

impl Deezer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Deezer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("limit", PAGE_SIZE.to_string()),
            ("index", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            if json_str(item, "type") != "track" {
                continue;
            }
            let title = json_str(item, "title");
            let url = https_url(&json_str(item, "link"));
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let artist = json_str(item, "artist.name");
            let album = json_str(item, "album.title");
            let content = [artist.clone(), album.clone(), title.clone()]
                .iter()
                .filter(|part| !part.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" - ");
            results.push(
                ResultBuilder::new(url, title)
                    .content(content)
                    .author(artist)
                    .thumbnail(json_str(item, "album.cover_medium"))
                    .template(Template::Music)
                    .metadata("album", album)
                    .metadata("duration", json_str(item, "duration"))
                    .metadata("track_id", json_str(item, "id"))
                    .metadata("preview", json_str(item, "preview"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Deezer still hands out plain http links; upstream upgrades them.
fn https_url(url: &str) -> String {
    match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": [
        {
          "id": 3135556,
          "readable": true,
          "title": "Harder, Better, Faster, Stronger",
          "duration": 224,
          "link": "http://www.deezer.com/track/3135556",
          "type": "track",
          "preview": "https://cdns-preview.deezer.com/stream/abc.mp3",
          "artist": {"id": 27, "name": "Daft Punk"},
          "album": {
            "id": 302127,
            "title": "Discovery",
            "cover_medium": "https://e-cdns-images.dzcdn.net/images/cover/abc/250x250-000000-80-0-0.jpg"
          }
        },
        {"id": 1, "title": "Not a track", "type": "artist", "link": "https://www.deezer.com/artist/1"}
      ]
    }"#;

    fn engine() -> Arc<Deezer> {
        Deezer::from_config(&EngineConfig::new("deezer", "deezer", "dz", &["music"])).unwrap()
    }

    #[test]
    fn parses_tracks_only() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("daft punk", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Harder, Better, Faster, Stronger");
                assert_eq!(result.url, "https://www.deezer.com/track/3135556");
                assert_eq!(result.template, Template::Music);
                assert_eq!(result.author.as_deref(), Some("Daft Punk"));
                assert_eq!(
                    result.content,
                    "Daft Punk - Discovery - Harder, Better, Faster, Stronger"
                );
                assert_eq!(
                    result.metadata.get("duration").map(String::as_str),
                    Some("224")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("daft punk", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with(SEARCH_URL));
        assert!(request.url.contains("limit=25"));
        assert!(request.url.contains("index=50"));
    }
}
