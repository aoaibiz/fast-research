//! Radio Browser engine, ported from searx/engines/radio_browser.py.
//!
//! Upstream resolves the server list with DNS and picks a random server; this
//! port talks to the documented de1 server (overridable with base_url). The
//! optional language and country station filters are not ported because they
//! need the CLDR language names of babel.
// engine-id: radio_browser

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["music", "radio"],
    shortcut: "rb",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://de1.api.radio-browser.info";
const PAGE_SIZE: u32 = 10;

/// Radio Browser station search.
pub struct RadioBrowser {
    meta: EngineMeta,
}

impl RadioBrowser {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
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
}

impl Engine for RadioBrowser {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("name", query.query_text()),
            ("order", "votes".to_string()),
            ("offset", (pageno.saturating_sub(1) * PAGE_SIZE).to_string()),
            ("limit", PAGE_SIZE.to_string()),
            ("hidebroken", "true".to_string()),
            ("reverse", "true".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/json/stations/search?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(stations) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for station in stations {
            let mut url = json_str(station, "homepage");
            if url.is_empty() {
                url = json_str(station, "url_resolved");
            }
            let title = json_str(station, "name");
            if title.is_empty() || !url.starts_with("http") {
                continue;
            }
            let mut content = Vec::new();
            let tags = tags_text(&json_str(station, "tags"));
            if !tags.is_empty() {
                content.push(tags);
            }
            for field in ["state", "country"] {
                let value = json_str(station, field).trim().to_string();
                if !value.is_empty() {
                    content.push(value);
                }
            }
            let mut builder = ResultBuilder::new(url, title).content(content.join(" | "));
            let favicon = https_url(&json_str(station, "favicon"));
            if !favicon.is_empty() {
                builder = builder.thumbnail(favicon);
            }
            let resolved = https_url(&json_str(station, "url_resolved"));
            if !resolved.is_empty() {
                builder = builder.metadata("iframe_src", resolved);
            }
            let codec = json_str(station, "codec");
            if !codec.is_empty() && !codec.eq_ignore_ascii_case("unknown") {
                builder = builder.metadata("codec", format!("{codec} radio"));
            }
            for (key, field) in [
                ("bitrate", "bitrate"),
                ("votes", "votes"),
                ("clicks", "clickcount"),
            ] {
                let value = json_str(station, field).trim().to_string();
                if !value.is_empty() {
                    builder = builder.metadata(key, value);
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream joins the comma separated tags with a comma and a space.
fn tags_text(raw: &str) -> String {
    raw.split(',').collect::<Vec<_>>().join(", ")
}

/// Upstream forces the stream and favicon URLs to https.
fn https_url(url: &str) -> String {
    url.trim().replace("http://", "https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {"name":"Radio One","homepage":"http://radio.example/",
       "url_resolved":"http://stream.radio.example/live",
       "favicon":"http://radio.example/favicon.png","tags":"news,music",
       "state":"Berlin","country":"Germany","codec":"MP3","bitrate":128,
       "votes":42,"clickcount":7},
      {"name":"No URL","homepage":"","url_resolved":""}
    ]"#;

    fn engine() -> Arc<RadioBrowser> {
        RadioBrowser::from_config(&EngineConfig::new(
            "radio browser",
            "radio_browser",
            "rb",
            &["music", "radio"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_stations() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("jazz", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "http://radio.example/");
                assert_eq!(result.title, "Radio One");
                assert_eq!(result.content, "news, music | Berlin | Germany");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://radio.example/favicon.png")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://stream.radio.example/live")
                );
                assert_eq!(
                    result.metadata.get("codec").map(String::as_str),
                    Some("MP3 radio")
                );
                assert_eq!(
                    result.metadata.get("bitrate").map(String::as_str),
                    Some("128")
                );
                assert_eq!(result.metadata.get("votes").map(String::as_str), Some("42"));
                assert_eq!(result.metadata.get("clicks").map(String::as_str), Some("7"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_station_search_request() {
        let query = Query::parse("jazz", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://de1.api.radio-browser.info/json/stations/search?")
        );
        assert!(request.url.contains("name=jazz"));
        assert!(request.url.contains("offset=10"));
        assert!(request.url.contains("limit=10"));
    }
}
