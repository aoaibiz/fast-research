//! Apple Maps engine, ported from searx/engines/apple_maps.py.

// engine-id: apple_maps

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["map"],
    shortcut: "apm",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

const SEARCH_URL: &str = "https://api.apple-mapkit.com/v1/search";
const MKJS_VERSION: &str = "5.72.53";

/// Apple Maps, through the MapKit search API.
pub struct AppleMaps {
    meta: EngineMeta,
}

impl AppleMaps {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for AppleMaps {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        let pairs = vec![
            ("q", query.query_text()),
            ("lang", language),
            ("mkjsVersion", MKJS_VERSION.to_string()),
        ];
        let endpoint = self.meta.search_url.as_deref().unwrap_or(SEARCH_URL);
        let mut request = EngineRequest::get(format!("{endpoint}?{}", query_string(&pairs)))
            .header("Accept", "application/json");
        // Upstream obtains a MapKit token at request time; the port takes a
        // static token from the settings (api_key or token).
        if let Some(token) = token(&self.meta) {
            request.set_header("Authorization", &format!("Bearer {token}"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "name");
            let url = json_str(item, "placecardUrl");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .template(Template::Map)
                .content(address(item))
                .metadata("type", json_str(item, "poiCategory"))
                .metadata("latitude", json_str(item, "center.lat"))
                .metadata("longitude", json_str(item, "center.lng"))
                .metadata("boundingbox", boundingbox(item));
            let telephone = json_str(item, "telephone");
            if !telephone.is_empty() {
                builder = builder.metadata("phone", format!("tel:{telephone}"));
            }
            let website = json_path(item, "urls")
                .and_then(|value| value.as_array())
                .and_then(|value| value.first())
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if !website.is_empty() {
                builder = builder.metadata("website", website);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn token(meta: &EngineMeta) -> Option<&str> {
    meta.api_key.as_deref().or_else(|| meta.opt_str("token"))
}

/// Upstream keeps the address as a structured object, we join its parts.
fn address(item: &serde_json::Value) -> String {
    let keys = [
        "thoroughfare",
        "subThoroughfare",
        "locality",
        "postCode",
        "country",
    ];
    keys.iter()
        .map(|key| json_str(item, key))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn boundingbox(item: &serde_json::Value) -> String {
    let Some(region) = json_path(item, "displayMapRegion") else {
        return String::new();
    };
    let keys = ["southLat", "northLat", "westLng", "eastLng"];
    keys.iter()
        .map(|key| json_str(region, key))
        .collect::<Vec<_>>()
        .join(",")
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"results": [
      {"name": "Eiffel Tower", "poiCategory": "Landmark", "placecardUrl": "https://maps.apple.com/place?address=Paris",
       "center": {"lat": 48.858, "lng": 2.294},
       "displayMapRegion": {"southLat": 48.85, "northLat": 48.86, "westLng": 2.29, "eastLng": 2.30},
       "telephone": "+33 1 44 11 23 23", "urls": ["https://www.toureiffel.paris/"],
       "thoroughfare": "Avenue Anatole France", "subThoroughfare": "5", "locality": "Paris",
       "postCode": "75007", "country": "France"},
      {"name": "No placecard"}
    ]}"##;

    fn engine() -> Arc<AppleMaps> {
        let mut config = EngineConfig::new("apple maps", "apple_maps", "apm", &["map"]);
        config.api_key = Some("secret-token".to_string());
        AppleMaps::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse(
            "eiffel tower",
            &crate::engine::EngineRegistry::default(),
            &[],
        )
    }

    fn meta(result: &crate::results::SearchResult, key: &str) -> String {
        result.metadata.get(key).cloned().unwrap_or_default()
    }

    #[test]
    fn parses_the_map_results() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Eiffel Tower");
                assert_eq!(result.url, "https://maps.apple.com/place?address=Paris");
                assert_eq!(result.template, Template::Map);
                assert_eq!(
                    result.content,
                    "Avenue Anatole France, 5, Paris, 75007, France"
                );
                assert_eq!(meta(result, "latitude"), "48.858");
                assert_eq!(meta(result, "longitude"), "2.294");
                assert_eq!(meta(result, "boundingbox"), "48.85,48.86,2.29,2.3");
                assert_eq!(meta(result, "type"), "Landmark");
                assert_eq!(meta(result, "phone"), "tel:+33 1 44 11 23 23");
                assert_eq!(meta(result, "website"), "https://www.toureiffel.paris/");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn sends_the_token_and_the_language() {
        let mut query = query();
        query.locale = "it-IT".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://api.apple-mapkit.com/v1/search?")
        );
        assert!(request.url.contains("q=eiffel%20tower"));
        assert!(request.url.contains("lang=it"));
        assert!(request.url.contains("mkjsVersion=5.72.53"));
        assert_eq!(
            request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
        // Without a configured token the header is simply not sent.
        let plain = EngineConfig::new("apple maps", "apple_maps", "apm", &["map"]);
        let request = AppleMaps::from_config(&plain)
            .unwrap()
            .build_request(&query, 1)
            .unwrap();
        assert!(request.headers.get("authorization").is_none());
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, SEARCH_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
