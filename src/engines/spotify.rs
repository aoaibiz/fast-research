//! Spotify engine, ported from searx/engines/spotify.py.
//!
//! Upstream obtains an access token with a client credentials POST inside its
//! request(); build_request is synchronous and cannot perform that round trip,
//! so the token is read from the access_token option (or api_key).
// engine-id: spotify

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
    shortcut: "stf",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://api.spotify.com";
/// Upstream requests 20 tracks per page.
const PAGE_SIZE: u32 = 20;

/// Spotify's official search API.
pub struct Spotify {
    meta: EngineMeta,
}

impl Spotify {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The bearer token of the search request.
    fn access_token(&self) -> Result<&str, EngineError> {
        self.meta
            .opt_str("access_token")
            .or(self.meta.api_key.as_deref())
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                EngineError::Message(
                    "spotify needs an access_token option: the client credentials exchange \
                     cannot run in build_request"
                        .to_string(),
                )
            })
    }
}

impl Engine for Spotify {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let token = self.access_token()?;
        let offset = (pageno.max(1) - 1) * PAGE_SIZE;
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(API_URL)
            .trim_end_matches('/');
        let url = format!(
            "{base}/v1/search?{}&type=track&offset={offset}",
            query_string(&[("q", query.query_text())])
        );
        Ok(EngineRequest::get(url)
            .header("Accept", "application/json")
            .header("Authorization", &format!("Bearer {token}")))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "tracks.items").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            if json_str(item, "type") != "track" {
                continue;
            }
            let title = json_str(item, "name");
            let url = json_str(item, "external_urls.spotify");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let artist = json_str(item, "artists.0.name");
            let album = json_str(item, "album.name");
            let content = format!("{artist} - {album} - {title}");
            let track_id = json_str(item, "id");
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .author(artist)
                .template(Template::Music)
                .metadata("album", album);
            if !track_id.is_empty() {
                builder = builder.metadata(
                    "iframe_src",
                    format!("https://embed.spotify.com/?uri=spotify:track:{track_id}"),
                );
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

    const FIXTURE: &str = r##"{
      "tracks": {
        "items": [
          {
            "type": "track",
            "id": "4cOdK2wGLETKBW3PvgPWqT",
            "name": "Never Gonna Give You Up",
            "external_urls": {"spotify": "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"},
            "artists": [{"name": "Rick Astley"}],
            "album": {"name": "Whenever You Need Somebody"}
          },
          {
            "type": "artist",
            "id": "artist1",
            "name": "Rick Astley",
            "external_urls": {"spotify": "https://open.spotify.com/artist/artist1"}
          }
        ]
      }
    }"##;

    fn engine() -> Arc<Spotify> {
        let mut config = EngineConfig::new("spotify", "spotify", "stf", &["music"]);
        config.api_key = Some("TOKEN".to_string());
        Spotify::from_config(&config).expect("spotify engine")
    }

    fn query() -> Query {
        Query::parse(
            "never gonna give you up",
            &crate::engine::EngineRegistry::default(),
            &[],
        )
    }

    #[test]
    fn parses_the_tracks() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Never Gonna Give You Up");
                assert_eq!(
                    result.url,
                    "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"
                );
                assert_eq!(
                    result.content,
                    "Rick Astley - Whenever You Need Somebody - Never Gonna Give You Up"
                );
                assert_eq!(result.template, Template::Music);
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://embed.spotify.com/?uri=spotify:track:4cOdK2wGLETKBW3PvgPWqT")
                );
            }
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn builds_the_paged_search_url() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://api.spotify.com/v1/search?q=never%20gonna%20give%20you%20up&type=track&offset=20"
        );
        assert_eq!(
            request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer TOKEN")
        );
    }

    #[test]
    fn a_missing_token_is_an_error() {
        let config = EngineConfig::new("spotify", "spotify", "stf", &["music"]);
        let engine = Spotify::from_config(&config).unwrap();
        assert!(engine.build_request(&query(), 1).is_err());
    }
}
