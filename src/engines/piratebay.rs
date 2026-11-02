//! Piratebay engine, ported from searx/engines/piratebay.py.
// engine-id: piratebay

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "tpb",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

/// The website, used for the description links.
const BASE_URL: &str = "https://thepiratebay.org";
/// The apibay.org JSON search endpoint.
const SEARCH_URL: &str = "https://apibay.org/q.php";
/// Trackers appended to every magnet link, like upstream.
const TRACKERS: &[&str] = &[
    "udp://tracker.coppersurfer.tk:6969/announce",
    "udp://tracker.opentrackr.org:1337",
    "udp://tracker.internetwarriors.net:1337/announce",
    "udp://tracker.leechers-paradise.org:6969/announce",
];

/// The Pirate Bay, through the apibay.org JSON API.
pub struct Piratebay {
    meta: EngineMeta,
}

/// The website base without a trailing slash.
fn site_base(meta: &EngineMeta) -> &str {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
}

/// The apibay category of the selected search category.
fn search_type(query: &Query) -> &'static str {
    match query.selected_categories().first().map(String::as_str) {
        Some("music") => "100",
        Some("videos") => "200",
        _ => "0",
    }
}

/// The magnet link upstream builds from the info hash and the trackers.
fn magnet_link(name: &str, info_hash: &str) -> Option<String> {
    if info_hash.is_empty() {
        return None;
    }
    Some(format!(
        "magnet:?xt=urn:btih:{}&dn={}&tr={}",
        info_hash,
        name,
        TRACKERS.join("&tr=")
    ))
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream's humanize_bytes (base 1024, two decimals).
fn humanize_bytes(mut size: f64) -> String {
    const SUFFIX: [&str; 5] = ["B ", "KB", "MB", "GB", "TB"];
    let mut position = 0usize;
    while size > 1024.0 && position < SUFFIX.len() {
        position += 1;
        size /= 1024.0;
    }
    let position = position.min(SUFFIX.len() - 1);
    format!("{size:.2} {}", SUFFIX[position])
}

impl Piratebay {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Piratebay {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("cat", search_type(query).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{SEARCH_URL}?{}",
            query_string(&pairs)
        )))
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
        if items.is_empty() || json_str(&items[0], "name") == "No results returned" {
            return Ok(Vec::new());
        }
        let mut results: Vec<(i64, EngineItem)> = Vec::new();
        for item in items {
            let name = json_str(item, "name");
            let id = json_str(item, "id");
            let url = format!("{}/description.php?id={}", site_base(&self.meta), id);
            if name.is_empty() || id.is_empty() || !is_http(&url) {
                continue;
            }
            let seeders = json_str(item, "seeders");
            let filesize = json_str(item, "size")
                .parse::<f64>()
                .map(humanize_bytes)
                .unwrap_or_default();
            let published = json_str(item, "added")
                .parse::<i64>()
                .ok()
                .and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0))
                .map(|date| date.to_rfc3339())
                .unwrap_or_default();
            let magnet = magnet_link(&name, &json_str(item, "info_hash")).unwrap_or_default();
            let builder = ResultBuilder::new(url, name)
                .metadata("seed", seeders.clone())
                .metadata("leech", json_str(item, "leechers"))
                .metadata("filesize", filesize)
                .metadata("magnetlink", magnet)
                .published_date(published);
            results.push((seeders.parse::<i64>().unwrap_or(0), builder.build()));
        }
        results.sort_by_key(|item| std::cmp::Reverse(item.0));
        Ok(results.into_iter().map(|(_, item)| item).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"[
      {"id": "42", "name": "Ubuntu 24.04", "info_hash": "ABC123", "seeders": "7", "leechers": "2", "size": "1048576", "added": "1700000000"},
      {"id": "43", "name": "Debian 12", "info_hash": "DEF456", "seeders": "120", "leechers": "4", "size": "1572864", "added": "1700000100"}
    ]"##;

    fn engine() -> Arc<Piratebay> {
        Piratebay::from_config(&EngineConfig::new(
            "piratebay",
            "piratebay",
            "tpb",
            &["files"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_and_sorts_by_seeders() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Debian 12");
                assert_eq!(result.url, "https://thepiratebay.org/description.php?id=43");
                assert_eq!(result.metadata["seed"], "120");
                assert_eq!(result.metadata["leech"], "4");
                assert_eq!(result.metadata["filesize"], "1.50 MB");
                assert!(result.metadata["magnetlink"].starts_with("magnet:?xt=urn:btih:DEF456"));
                assert!(
                    result
                        .published_date
                        .as_deref()
                        .unwrap()
                        .contains("2023-11")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => assert_eq!(result.title, "Ubuntu 24.04"),
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn empty_api_answer_yields_no_results() {
        let body = r##"[{"id": "0", "name": "No results returned", "info_hash": ""}]"##;
        let response = EngineResponse::new(200, SEARCH_URL, body.as_bytes().to_vec());
        let query = Query::parse("nothing", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }

    #[test]
    fn request_uses_the_apibay_category() {
        let query = Query::parse("jazz", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://apibay.org/q.php?"));
        assert!(request.url.contains("q=jazz"));
        assert!(request.url.contains("cat=0"));
        let mut music = query.clone();
        music.categories = vec!["music".to_string()];
        let request = engine().build_request(&music, 1).unwrap();
        assert!(request.url.contains("cat=100"));
    }
}
