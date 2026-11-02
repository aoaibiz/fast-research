//! OpenStreetMap engine, ported from `searx/engines/openstreetmap.py` (the
//! Nominatim part; the Wikidata and route extensions are not ported).

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
    shortcut: "osm",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://nominatim.openstreetmap.org/search";
const PAGE_SIZE: u32 = 10;
const RESULT_URL: &str = "https://openstreetmap.org/{osm_type}/{osm_id}";
/// Nominatim requires an identifying User-Agent.
const USER_AGENT: &str = "rustxng/0.1 (+https://github.com/searxng/searxng)";

/// OpenStreetMap's Nominatim search API.
pub struct Openstreetmap {
    meta: EngineMeta,
}

impl Openstreetmap {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Openstreetmap {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("format", "jsonv2".to_string()),
            ("addressdetails", "1".to_string()),
            ("limit", PAGE_SIZE.to_string()),
        ];
        let mut request = EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json");
        let language = locale_language(&query.locale);
        if language != "all" {
            request.set_header("Accept-Language", &language);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = place_title(item);
            if title.is_empty() {
                continue;
            }
            let osm_type = json_str(item, "osm_type");
            let osm_id = json_str(item, "osm_id");
            if osm_type.is_empty() || osm_id.is_empty() {
                continue;
            }
            let url = RESULT_URL
                .replace("{osm_type}", &osm_type)
                .replace("{osm_id}", &osm_id);
            if !is_http_url(&url) {
                continue;
            }
            let display_name = json_str(item, "display_name");
            results.push(
                ResultBuilder::new(url, title)
                    .content(display_name.clone())
                    .template(Template::Map)
                    .metadata("latitude", json_str(item, "lat"))
                    .metadata("longitude", json_str(item, "lon"))
                    .metadata("osm_type", osm_type)
                    .metadata("osm_id", osm_id)
                    .metadata("address", display_name)
                    .metadata("category", json_str(item, "category"))
                    .metadata("type", json_str(item, "type"))
                    .metadata("place_id", json_str(item, "place_id"))
                    .metadata("boundingbox", boundingbox(item))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Nominatim's name, falling back to the first part of display_name.
fn place_title(item: &serde_json::Value) -> String {
    let name = json_str(item, "name");
    if !name.is_empty() {
        return name;
    }
    json_str(item, "display_name")
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The boundingbox array joined with commas.
fn boundingbox(item: &serde_json::Value) -> String {
    json_path(item, "boundingbox")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {
        "place_id": 123456,
        "osm_type": "node",
        "osm_id": 456789,
        "lat": "48.8582599",
        "lon": "2.2945006",
        "display_name": "Eiffel Tower, 5, Avenue Anatole France, Paris, France",
        "name": "Eiffel Tower",
        "category": "tourism",
        "type": "attraction",
        "boundingbox": ["48.857", "48.859", "2.293", "2.296"]
      }
    ]"#;

    fn engine() -> Arc<Openstreetmap> {
        Openstreetmap::from_config(&EngineConfig::new(
            "openstreetmap",
            "openstreetmap",
            "osm",
            &["map"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_nominatim_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse(
            "eiffel tower",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Eiffel Tower");
                assert_eq!(result.url, "https://openstreetmap.org/node/456789");
                assert_eq!(result.template, Template::Map);
                assert_eq!(
                    result.metadata.get("latitude").map(String::as_str),
                    Some("48.8582599")
                );
                assert_eq!(
                    result.metadata.get("longitude").map(String::as_str),
                    Some("2.2945006")
                );
                assert_eq!(
                    result.metadata.get("boundingbox").map(String::as_str),
                    Some("48.857,48.859,2.293,2.296")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn sets_the_user_agent_and_does_not_page() {
        let query = Query::parse("paris", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 4).unwrap();
        assert_eq!(
            request
                .headers
                .get("user-agent")
                .and_then(|value| value.to_str().ok()),
            Some(USER_AGENT)
        );
        assert!(!request.url.contains("page="));
        assert!(request.url.contains("limit=10"));
    }
}
