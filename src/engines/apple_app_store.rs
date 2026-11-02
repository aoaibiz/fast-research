//! Apple App Store engine, ported from `searx/engines/apple_app_store.py`.
// engine-id: apple_app_store

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "apps"],
    shortcut: "aps",
    paging: false,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// The public iTunes Search API (upstream's `search_url`).
const SEARCH_URL: &str = "https://itunes.apple.com/search";

/// Apple App Store, through the iTunes Search API.
pub struct AppleAppStore {
    meta: EngineMeta,
}

impl AppleAppStore {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for AppleAppStore {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // Upstream sends `explicit=Yes` only when safesearch is off.
        let explicit = if query.safesearch > 0 { "No" } else { "Yes" };
        let pairs = vec![
            ("term", query.query_text()),
            ("media", "software".to_string()),
            ("explicit", explicit.to_string()),
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
        let Some(items) = json_path(&json, "results").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "trackName");
            let url = json_str(item, "trackViewUrl");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "description"))
                .thumbnail(json_str(item, "artworkUrl100"))
                .author(json_str(item, "sellerName"));
            let release = json_str(item, "currentVersionReleaseDate");
            if !release.is_empty() {
                builder = builder.published_date(first_ten(&release));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn first_ten(value: &str) -> String {
    value.chars().take(10).collect()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "resultCount": 1,
      "results": [
        {"trackName": "Rust Playground", "trackViewUrl": "https://apps.apple.com/app/id123",
         "description": "Learn Rust", "artworkUrl100": "https://is1.apple.com/art.png",
         "sellerName": "Jane Doe", "currentVersionReleaseDate": "2024-04-01T10:00:00Z"}
      ]
    }"#;

    fn engine() -> Arc<AppleAppStore> {
        AppleAppStore::from_config(&EngineConfig::new(
            "apple_app_store",
            "apple_app_store",
            "aps",
            &["it", "apps"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_app_results() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust Playground");
                assert_eq!(result.url, "https://apps.apple.com/app/id123");
                assert_eq!(result.author.as_deref(), Some("Jane Doe"));
                assert_eq!(result.published_date.as_deref(), Some("2024-04-01"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_itunes_request() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.safesearch = 1;
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://itunes.apple.com/search?"));
        assert!(request.url.contains("term=rust"));
        assert!(request.url.contains("media=software"));
        assert!(request.url.contains("explicit=No"));
    }
}
