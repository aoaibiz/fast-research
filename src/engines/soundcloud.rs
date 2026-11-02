//! SoundCloud engine, ported from `searx/engines/soundcloud.py`.
//!
//! Search uses the private API of the SoundCloud web frontend.  Upstream scrapes
//! a guest `client_id` from the homepage; `build_request` is synchronous and
//! cannot fetch it, so the id has to be configured (`client_id` or `api_key`).
//! Without one the engine is disabled.
// engine-id: soundcloud

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music"],
    shortcut: "sc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://api-v2.soundcloud.com/search";
const FACET: &str = "model";
const RESULTS_PER_PAGE: u32 = 10;

/// A SoundCloud client id.
pub struct Soundcloud {
    meta: EngineMeta,
}

impl Soundcloud {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The guest client id (`client_id` first, then `api_key`).
    fn client_id(&self) -> Option<String> {
        self.meta
            .opt_str("client_id")
            .map(str::to_string)
            .or_else(|| self.meta.api_key.clone())
            .filter(|value| !value.trim().is_empty())
    }
}

impl Engine for Soundcloud {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let Some(client_id) = self.client_id() else {
            return Err(EngineError::Message(
                "soundcloud: a guest client_id (or api_key) is required".into(),
            ));
        };
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            (
                "offset",
                ((pageno.max(1) - 1) * RESULTS_PER_PAGE).to_string(),
            ),
            ("limit", RESULTS_PER_PAGE.to_string()),
            ("facet", FACET.to_string()),
            ("client_id", client_id),
            ("app_locale", app_locale(&query.engine_language())),
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
        let Some(items) = json_path(&json, "collection").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let kind = json_str(item, "kind");
            if kind != "track" && kind != "playlist" {
                continue;
            }
            let url = json_str(item, "permalink_url");
            let title = json_str(item, "title");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let mut content_parts = Vec::new();
            let description = json_str(item, "description");
            if !description.is_empty() {
                content_parts.push(description);
            }
            let label = json_str(item, "label_name");
            if !label.is_empty() {
                content_parts.push(label);
            }
            let thumbnail = {
                let artwork = json_str(item, "artwork_url");
                if artwork.is_empty() {
                    json_str(item, "user.avatar_url")
                } else {
                    artwork
                }
            };
            let plays = json_path(item, "playback_count")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let mut builder = ResultBuilder::new(url, title)
                .content(content_parts.join(" / "))
                .published_date(json_str(item, "last_modified"))
                .author(json_str(item, "user.full_name"))
                .thumbnail(thumbnail);
            if plays > 0 {
                // Upstream stores the raw `playback_count`.
                builder = builder.metadata("views", plays.to_string());
            }
            let duration = json_path(item, "duration")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if duration > 0 {
                builder = builder.metadata("length", format_duration(duration / 1000));
            }
            let uri = json_str(item, "uri");
            if !uri.is_empty() {
                builder = builder.metadata(
                    "iframe_src",
                    format!("https://w.soundcloud.com/player/?url={}", encode(&uri)),
                );
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream's `app_locale_map`.
fn app_locale(language: &str) -> String {
    let base = language.split('-').next().unwrap_or("en");
    match base {
        "de" => "de",
        "en" => "en",
        "es" => "es",
        "fr" | "oc" => "fr",
        "it" => "it",
        "nl" => "nl",
        "pl" | "szl" => "pl",
        "pt" | "pap" => "pt_BR",
        "sv" => "sv",
        _ => "en",
    }
    .to_string()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn format_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "collection": [
        {
          "kind": "track",
          "permalink_url": "https://soundcloud.com/artist/a-track",
          "uri": "https://api.soundcloud.com/tracks/123",
          "title": "A track",
          "description": "A description",
          "label_name": "A label",
          "last_modified": "2024-01-01T00:00:00Z",
          "artwork_url": "https://i1.sndcdn.com/artworks-abc-large.jpg",
          "duration": 125000,
          "playback_count": 12345,
          "user": {"full_name": "An artist", "avatar_url": "https://i1.sndcdn.com/avatars-abc-large.jpg"}
        }
      ]
    }"#;

    fn engine() -> Arc<Soundcloud> {
        let mut config = EngineConfig::new("soundcloud", "soundcloud", "sc", &["music"]);
        config.extra.insert(
            "client_id".to_string(),
            toml::Value::String("abc123".to_string()),
        );
        Soundcloud::from_config(&config).unwrap()
    }

    #[test]
    fn parses_tracks() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("track", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A track");
                assert_eq!(result.url, "https://soundcloud.com/artist/a-track");
                assert_eq!(result.content, "A description / A label");
                assert_eq!(result.author.as_deref(), Some("An artist"));
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("02:05")
                );
                assert_eq!(
                    result.metadata.get("views").map(String::as_str),
                    Some("12345")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("track", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api-v2.soundcloud.com/search?")
        );
        assert!(request.url.contains("offset=10"));
        assert!(request.url.contains("client_id=abc123"));
        assert!(request.url.contains("facet=model"));
    }

    #[test]
    fn without_a_client_id_the_engine_is_disabled() {
        let config = EngineConfig::new("soundcloud", "soundcloud", "sc", &["music"]);
        let engine = Soundcloud::from_config(&config).unwrap();
        let query = Query::parse("track", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine.build_request(&query, 1),
            Err(EngineError::Message(_))
        ));
    }
}
