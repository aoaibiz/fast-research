//! SensCritique engine, ported from searx/engines/senscritique.py.
// engine-id: senscritique

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["movies"],
    shortcut: "scr",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const GRAPHQL_URL: &str = "https://apollo.senscritique.com/";
const PAGE_SIZE: i64 = 16;

const GRAPHQL_QUERY: &str = concat!(
    "query SearchProductExplorer($query: String, $offset: Int, $limit: Int, ",
    "$sortBy: SearchProductExplorerSort) { searchProductExplorer(query: $query ",
    "filters: [] sortBy: $sortBy offset: $offset limit: $limit) { items { category ",
    "dateRelease duration id originalTitle rating title url yearOfProduction ",
    "medias { picture } countries { name } genresInfos { label } directors { name } ",
    "stats { ratingCount } } } }"
);

/// SensCritique, the French movie community.
pub struct Senscritique {
    meta: EngineMeta,
}

impl Senscritique {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Senscritique {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = i64::from(pageno.max(1) - 1) * PAGE_SIZE;
        let body = serde_json::json!({
            "operationName": "SearchProductExplorer",
            "variables": {
                "offset": offset,
                "limit": PAGE_SIZE,
                "query": query.query_text(),
                "sortBy": "RELEVANCE",
            },
            "query": GRAPHQL_QUERY,
        });
        let url = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| GRAPHQL_URL.to_string());
        Ok(EngineRequest::post(url)
            .header("Content-Type", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data.searchProductExplorer.items")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            if let Some(entry) = parse_item(item) {
                results.push(entry);
            }
        }
        Ok(results)
    }
}

fn parse_item(item: &serde_json::Value) -> Option<EngineItem> {
    let title = json_str(item, "title");
    if title.is_empty() {
        return None;
    }
    let year = json_str(item, "yearOfProduction");
    let original_title = json_str(item, "originalTitle");
    let path = json_str(item, "url");
    let url = if path.starts_with("http") {
        path
    } else {
        format!("https://www.senscritique.com{path}")
    };
    if !is_http(&url) {
        return None;
    }
    let display_title = if year.is_empty() {
        title.clone()
    } else {
        format!("{title} ({year})")
    };
    let mut parts: Vec<String> = Vec::new();
    let category = json_str(item, "category");
    if !category.is_empty() {
        parts.push(category);
    }
    if !original_title.is_empty() && original_title != title {
        parts.push(format!("Original title: {original_title}"));
    }
    push_names(&mut parts, item, "directors", "name", "Director(s): ");
    push_names(&mut parts, item, "countries", "name", "Country: ");
    push_names(&mut parts, item, "genresInfos", "label", "Genre(s): ");
    let minutes = json_path(item, "duration")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        / 60;
    if minutes > 0 {
        parts.push(format!("Duration: {minutes} min"));
    }
    let rating = json_path(item, "rating")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| *value != 0.0);
    let votes = json_path(item, "stats.ratingCount")
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value != 0);
    if let (Some(rating), Some(votes)) = (rating, votes) {
        parts.push(format!(
            "Rating: {}/10 ({votes} votes)",
            rating_text(rating)
        ));
    }
    let mut builder = ResultBuilder::new(url, display_title).content(parts.join(" | "));
    let thumbnail = json_str(item, "medias.picture");
    if is_http(&thumbnail) {
        builder = builder.thumbnail(thumbnail);
    }
    Some(builder.build())
}

fn push_names(
    parts: &mut Vec<String>,
    item: &serde_json::Value,
    path: &str,
    key: &str,
    label: &str,
) {
    let Some(values) = json_path(item, path).and_then(serde_json::Value::as_array) else {
        return;
    };
    let names: Vec<&str> = values
        .iter()
        .filter_map(|entry| entry.get(key))
        .filter_map(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .collect();
    if !names.is_empty() {
        parts.push(format!("{label}{}", names.join(", ")));
    }
}

/// Python prints whole floats with a trailing .0, Rust does not.
fn rating_text(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"data":{"searchProductExplorer":{"items":[
      {"title":"Arrival","originalTitle":"Arrival","yearOfProduction":2016,"url":"/film/arrival/123",
       "duration":6960,"rating":7.9,"category":"Film","medias":{"picture":"https://media.example.com/a.jpg"},
       "countries":[{"name":"USA"}],"genresInfos":[{"label":"Science fiction"}],
       "directors":[{"name":"Denis Villeneuve"}],"stats":{"ratingCount":12345}},
      {"title":"Le Samourai","yearOfProduction":1967,"url":"/film/le-samourai/9",
       "duration":0,"rating":8,"category":"Film","stats":{"ratingCount":100}},
      {"title":"","url":"/film/broken/1"}
    ]}}}"##;

    fn engine() -> Arc<Senscritique> {
        Senscritique::from_config(&EngineConfig::new(
            "senscritique",
            "senscritique",
            "scr",
            &["movies"],
        ))
        .unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_movie_items() {
        let response = EngineResponse::new(200, GRAPHQL_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("arrival")).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(first.title, "Arrival (2016)");
        assert_eq!(first.url, "https://www.senscritique.com/film/arrival/123");
        assert_eq!(
            first.content,
            "Film | Director(s): Denis Villeneuve | Country: USA | Genre(s): Science fiction | Duration: 116 min | Rating: 7.9/10 (12345 votes)"
        );
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://media.example.com/a.jpg")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(second.title, "Le Samourai (1967)");
        assert_eq!(second.content, "Film | Rating: 8.0/10 (100 votes)");
        assert!(second.thumbnail.is_none());
    }

    #[test]
    fn builds_the_graphql_request() {
        let request = engine().build_request(&query("arrival"), 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, GRAPHQL_URL);
        let body = request.json.expect("a JSON body");
        assert_eq!(body["operationName"], "SearchProductExplorer");
        assert_eq!(
            body["variables"],
            serde_json::json!({"offset": 16, "limit": 16, "query": "arrival", "sortBy": "RELEVANCE"})
        );
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("SearchProductExplorer")
        );
    }

    #[test]
    fn missing_items_yield_no_results() {
        let response = EngineResponse::new(200, GRAPHQL_URL, b"{}".to_vec());
        assert!(engine().parse(&response, &query("x")).unwrap().is_empty());
    }
}
