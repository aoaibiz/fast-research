//! Moviepilot engine, ported from searx/engines/moviepilot.py.
//!
//! Moviepilot is a German movie database. Queries made of filter terms such
//! as genre-actionfilm use the discovery API, every other query uses the
//! internal suggestion search.

// engine-id: moviepilot

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["movies"],
    shortcut: "mp",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.moviepilot.de";
const IMAGE_URL: &str = "https://assets.cdn.moviepilot.de/files";
const FILTER_TYPES: &[&str] = &[
    "fsk",
    "genre",
    "jahr",
    "jahrzehnt",
    "land",
    "online",
    "stimmung",
    "person",
];

/// Moviepilot, the German movie database.
pub struct Moviepilot {
    meta: EngineMeta,
}

impl Moviepilot {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The filter terms of a discovery query, e.g. genre-actionfilm.
fn discovery_filters(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|part| match part.split_once('-') {
            Some((category, _)) => FILTER_TYPES.contains(&category),
            None => false,
        })
        .map(str::to_string)
        .collect()
}

fn thumbnail_url(image: &str, filename: &str) -> String {
    if image.is_empty() || filename.is_empty() {
        String::new()
    } else {
        format!("{IMAGE_URL}/{image}/fill/155/223/{filename}")
    }
}

impl Engine for Moviepilot {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = base_url(&self.meta);
        let filters = discovery_filters(&query.query_text());
        let mut url = if filters.is_empty() {
            let pairs = vec![
                ("q", query.query_text()),
                ("page", pageno.max(1).to_string()),
                ("type", "suggest".to_string()),
            ];
            format!("{base}/api/search?{}", query_string(&pairs))
        } else {
            let pairs = vec![
                ("page", pageno.max(1).to_string()),
                ("order", "beste".to_string()),
            ];
            format!("{base}/api/discovery?{}", query_string(&pairs))
        };
        for filter in &filters {
            url.push_str("&filters[]=");
            url.push_str(filter);
        }
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let discovery = !discovery_filters(&query.query_text()).is_empty();
        let items = if discovery {
            json_path(&json, "results").and_then(|value| value.as_array())
        } else {
            json.as_array()
        };
        let Some(items) = items else {
            return Ok(Vec::new());
        };
        let base = base_url(&self.meta);
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            let (url, content, metadata, thumbnail) = if discovery {
                let path = json_str(item, "path");
                if path.is_empty() {
                    continue;
                }
                let url = if is_http(&path) {
                    path
                } else {
                    format!("{base}{path}")
                };
                if !is_http(&url) {
                    continue;
                }
                let content = ["abstract", "summary"]
                    .iter()
                    .map(|key| strip_tags(&json_str(item, key)))
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join(" | ");
                let meta_short = strip_tags(&json_str(item, "meta_short"));
                let image = json_str(item, "image");
                let filename = json_str(item, "image_filename");
                (url, content, meta_short, thumbnail_url(&image, &filename))
            } else {
                let url = json_str(item, "url");
                if !is_http(&url) {
                    continue;
                }
                let content = ["class", "info", "more"]
                    .iter()
                    .map(|key| json_str(item, key))
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                (url, content, String::new(), json_str(item, "image"))
            };
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .metadata("meta_short", metadata);
            if is_http(&thumbnail) {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISCOVERY: &str = r##"{
      "results": [{"title": "Film", "path": "/filme/film", "abstract": "A <b>film</b>",
        "summary": "Summary", "meta_short": "1999", "image": "abc", "image_filename": "poster.jpg"}]
    }"##;

    const SEARCH: &str = r##"[{"title": "Film", "url": "https://www.moviepilot.de/movies/film",
      "class": "Movie", "info": "1999", "more": "Action",
      "image": "https://assets.cdn.moviepilot.de/files/abc/poster.jpg"}]"##;

    fn engine() -> Arc<Moviepilot> {
        Moviepilot::from_config(&EngineConfig::new(
            "moviepilot",
            "moviepilot",
            "mp",
            &["movies"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_both_api_urls() {
        let discovery = engine()
            .build_request(&query("genre-actionfilm jahr-2020er"), 2)
            .unwrap();
        assert!(discovery.url.contains("/api/discovery?"));
        assert!(discovery.url.contains("page=2"));
        assert!(discovery.url.contains("order=beste"));
        assert!(discovery.url.contains("filters[]=genre-actionfilm"));
        assert!(discovery.url.contains("filters[]=jahr-2020er"));
        let search = engine().build_request(&query("Tom Cruise"), 1).unwrap();
        assert!(search.url.contains("/api/search?"));
        assert!(search.url.contains("q=Tom%20Cruise"));
        assert!(search.url.contains("type=suggest"));
    }

    #[test]
    fn parses_both_result_shapes() {
        let response = EngineResponse::new(200, BASE_URL, DISCOVERY.as_bytes().to_vec());
        let items = engine()
            .parse(&response, &query("genre-actionfilm"))
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.moviepilot.de/filme/film");
                assert_eq!(result.content, "A film | Summary");
                assert_eq!(
                    result.metadata.get("meta_short").map(String::as_str),
                    Some("1999")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://assets.cdn.moviepilot.de/files/abc/fill/155/223/poster.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
        let response = EngineResponse::new(200, BASE_URL, SEARCH.as_bytes().to_vec());
        let items = engine().parse(&response, &query("Film")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.content, "Movie, 1999, Action");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://assets.cdn.moviepilot.de/files/abc/poster.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn missing_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine()
            .parse(&response, &query("genre-actionfilm"))
            .unwrap();
        assert!(items.is_empty());
    }
}
