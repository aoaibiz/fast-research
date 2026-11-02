//! Steam engine, ported from searx/engines/steam.py.
//!
//! The store search API answers JSON. Upstream declares no category, which
//! places the engine in the general tab.
// engine-id: steam

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "stm",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://store.steampowered.com";

/// Steam store search.
pub struct Steam {
    meta: EngineMeta,
}

impl Steam {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Steam {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("term", query.query_text()),
            ("cc", "us".to_string()),
            ("l", "en".to_string()),
        ];
        let url = format!(
            "{}/api/storesearch/?{}",
            self.base_url(),
            query_string(&pairs)
        );
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for item in items {
            let app_id = json_str(item, "id");
            let title = json_str(item, "name");
            if app_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{base}/app/{app_id}");
            if !is_http_url(&url) {
                continue;
            }
            let mut currency = json_str(item, "price.currency");
            if currency.is_empty() {
                currency = "USD".to_string();
            }
            let cents = json_path(item, "price.final")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            let mut platforms: Vec<String> = Vec::new();
            if let Some(map) = json_path(item, "platforms").and_then(|value| value.as_object()) {
                for (platform, supported) in map {
                    if supported.as_bool().unwrap_or(false) {
                        platforms.push(platform.to_string());
                    }
                }
            }
            platforms.sort();
            let mut content = vec![format!("Price: {:.2} {currency}", cents / 100.0)];
            if !platforms.is_empty() {
                content.push(format!("Platforms: {}", platforms.join(", ")));
            }
            let mut builder = ResultBuilder::new(url, title).content(content.join(" | "));
            let thumbnail = json_str(item, "tiny_image");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
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

    const FIXTURE: &str = r#"{"items": [
      {"id": 730, "name": "Counter-Strike 2", "price": {"currency": "USD", "final": 1999},
       "platforms": {"windows": true, "mac": false, "linux": true},
       "tiny_image": "https://cdn.akamai.steamstatic.com/steam/apps/730/capsule.jpg"},
      {"id": 570, "name": "", "price": {"currency": "USD", "final": 0}, "platforms": {}}
    ]}"#;

    fn engine() -> Arc<Steam> {
        Steam::from_config(&EngineConfig::new("steam", "steam", "stm", &["general"])).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_store_items() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("counter strike")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Counter-Strike 2");
                assert_eq!(result.url, "https://store.steampowered.com/app/730");
                assert_eq!(
                    result.content,
                    "Price: 19.99 USD | Platforms: linux, windows"
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.akamai.steamstatic.com/steam/apps/730/capsule.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query("counter strike"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://store.steampowered.com/api/storesearch/?term=counter%20strike&cc=us&l=en"
        );
    }

    #[test]
    fn missing_items_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query("x")).unwrap().is_empty());
    }
}
