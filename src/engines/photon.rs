//! Photon engine, ported from `searx/engines/photon.py`.

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
    shortcut: "ph",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://photon.komoot.io/api/";
const PAGE_SIZE: u32 = 10;
const RESULT_URL: &str = "https://openstreetmap.org/{osm_type}/{osm_id}";
const USER_AGENT: &str = "rustxng/0.1 (+https://github.com/searxng/searxng)";
/// Languages the Photon index actually carries (upstream list).
const PHOTON_LANGUAGES: &[&str] = &["de", "en", "fr", "it"];

/// Photon, the Komoot geocoder.
pub struct Photon {
    meta: EngineMeta,
}

impl Photon {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Photon {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("q", query.query_text()), ("limit", PAGE_SIZE.to_string())];
        let mut request = EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json");
        let language = locale_language(&query.locale);
        if PHOTON_LANGUAGES.contains(&language.as_str()) {
            request.url.push_str(&format!("&lang={language}"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(features) = json_path(&json, "features").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for feature in features {
            let Some(properties) = feature.get("properties") else {
                continue;
            };
            if !properties.is_object() {
                continue;
            }
            let title = json_str(properties, "name");
            if title.is_empty() {
                continue;
            }
            let osm_type = match json_str(properties, "osm_type").as_str() {
                "N" => "node",
                "W" => "way",
                "R" => "relation",
                _ => continue,
            };
            let osm_id = json_str(properties, "osm_id");
            let url = RESULT_URL
                .replace("{osm_type}", osm_type)
                .replace("{osm_id}", &osm_id);
            if !is_http_url(&url) {
                continue;
            }
            let coordinates =
                json_path(feature, "geometry.coordinates").and_then(serde_json::Value::as_array);
            let longitude = coordinates
                .and_then(|values| values.first())
                .map(number)
                .unwrap_or_default();
            let latitude = coordinates
                .and_then(|values| values.get(1))
                .map(number)
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .content(address_text(properties))
                    .template(Template::Map)
                    .metadata("latitude", latitude)
                    .metadata("longitude", longitude)
                    .metadata("osm_type", osm_type)
                    .metadata("osm_id", osm_id)
                    .metadata("osm_key", json_str(properties, "osm_key"))
                    .metadata("osm_value", json_str(properties, "osm_value"))
                    .metadata("boundingbox", boundingbox(properties, coordinates))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

fn number(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

fn first_non_empty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .cloned()
        .unwrap_or_default()
}

/// A one line address, mirroring upstream's address dictionary.
fn address_text(properties: &serde_json::Value) -> String {
    let locality = first_non_empty(&[
        json_str(properties, "city"),
        json_str(properties, "town"),
        json_str(properties, "village"),
    ]);
    let parts = [
        json_str(properties, "housenumber"),
        json_str(properties, "street"),
        locality,
        json_str(properties, "postcode"),
        json_str(properties, "country"),
    ];
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// The extent in Nominatim order, falling back to the point itself.
fn boundingbox(
    properties: &serde_json::Value,
    coordinates: Option<&Vec<serde_json::Value>>,
) -> String {
    if let Some(extent) = json_path(properties, "extent").and_then(serde_json::Value::as_array) {
        if extent.len() >= 4 {
            return [
                number(&extent[3]),
                number(&extent[1]),
                number(&extent[0]),
                number(&extent[2]),
            ]
            .join(",");
        }
    }
    if let Some(values) = coordinates {
        if values.len() >= 2 {
            let latitude = number(&values[1]);
            let longitude = number(&values[0]);
            return format!("{latitude},{latitude},{longitude},{longitude}");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "type": "FeatureCollection",
      "features": [
        {
          "type": "Feature",
          "geometry": {"type": "Point", "coordinates": [13.3888599, 52.5170365]},
          "properties": {
            "osm_id": 123456,
            "osm_type": "W",
            "name": "Berlin",
            "city": "Berlin",
            "country": "Germany",
            "postcode": "10117",
            "street": "Unter den Linden",
            "housenumber": "1",
            "osm_key": "place",
            "osm_value": "city",
            "extent": [13.1, 52.7, 13.8, 52.3]
          }
        }
      ]
    }"#;

    fn engine() -> Arc<Photon> {
        Photon::from_config(&EngineConfig::new("photon", "photon", "ph", &["map"])).unwrap()
    }

    #[test]
    fn parses_feature_collections() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("berlin", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Berlin");
                assert_eq!(result.url, "https://openstreetmap.org/way/123456");
                assert_eq!(result.template, Template::Map);
                assert_eq!(
                    result.metadata.get("latitude").map(String::as_str),
                    Some("52.5170365")
                );
                assert_eq!(
                    result.metadata.get("longitude").map(String::as_str),
                    Some("13.3888599")
                );
                assert_eq!(
                    result.metadata.get("boundingbox").map(String::as_str),
                    Some("52.3,52.7,13.1,13.8")
                );
                assert_eq!(
                    result.content,
                    "1, Unter den Linden, Berlin, 10117, Germany"
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn forwards_a_supported_language() {
        let mut query = Query::parse("berlin", &crate::engine::EngineRegistry::default(), &[]);
        query.locale = "de-DE".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.contains("&lang=de"));
        assert!(request.url.contains("limit=10"));
    }
}
