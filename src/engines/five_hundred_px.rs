//! 500px engine, ported from `searx/engines/500px.py` (stock images).
// engine-id: 500px

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["stock images"],
    shortcut: "500px",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://500px.com";
const API_URL: &str = "https://api-neo.500px.com";
/// The web UI requests `first: 30` (the GraphQL query hardcodes it).
const RESULTS_PER_PAGE: i64 = 30;

/// The GraphQL query of upstream's `SXNG_query`.
const SEARCH_QUERY: &str = r#"query searchResource(
  $keyword: String!
  $type: SearchType = TEXT
  $first: Int!
  $after: String
  $resourceTypes: [SearchResourceType!]
  $categories: [String!]
  $equipments: [String!]
  $downloadable: PhotoDownloadable
  $sort: SearchSortOption = RELEVANCE
  $excludeNsfw: Boolean
) {
  searchResource(
    keyword: $keyword
    type: $type
    first: $first
    after: $after
    resourceTypes: $resourceTypes
    categories: $categories
    equipments: $equipments
    downloadable: $downloadable
    sort: $sort
    excludeNsfw: $excludeNsfw
  ) {
    edges {
      cursor
      node {
        __typename
        ... on Photo {
          id
          title
          description
          urls {
            size_600
            size_1024
            size_2048
            size_4k
            __typename
          }
          uploader {
            displayName
            __typename
          }
          isNsfw
          width
          height
          uploadedAt
          __typename
        }
      }
      __typename
    }
    __typename
  }
}"#;

/// 500px, a photo sharing network.
pub struct FiveHundredPx {
    meta: EngineMeta,
    results_per_page: i64,
}

impl FiveHundredPx {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            results_per_page: config
                .opt_i64("results_per_page")
                .unwrap_or(RESULTS_PER_PAGE),
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for FiveHundredPx {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // The cursor is the base64 hash of "pos-<offset>", e.g. "pos-29".
        let offset = ((pageno.max(1) as i64) - 1) * self.results_per_page - 1;
        let cursor = base64_encode(&format!("pos-{offset}"));
        let body = serde_json::json!({
            "operationName": "searchResource",
            "variables": {
                "after": cursor,
                "first": self.results_per_page,
                "keyword": query.query_text(),
                "resourceTypes": ["PHOTO"],
                "sort": "RELEVANCE",
                "type": "TEXT",
            },
            "query": SEARCH_QUERY,
        });
        Ok(EngineRequest::post(format!("{API_URL}/graphql"))
            .json_body(body)
            .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(edges) =
            json_path(&json, "data.searchResource.edges").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for edge in edges {
            let node = json_path(edge, "node").unwrap_or(edge);
            let title = json_str(node, "title");
            let id = json_str(node, "id");
            if title.is_empty() || id.is_empty() {
                continue;
            }
            // Upstream takes the first and the last of the size_* URLs; the JSON
            // order is ascending quality.
            let urls = json_path(node, "urls")
                .and_then(serde_json::Value::as_object)
                .map(|object| {
                    let mut sizes: Vec<(&String, &serde_json::Value)> = object.iter().collect();
                    sizes.sort_by(|left, right| left.0.cmp(right.0));
                    sizes
                        .into_iter()
                        .map(|(_, value)| value.as_str().unwrap_or_default().to_string())
                        .filter(|url| !url.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if urls.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(format!("{BASE_URL}/photo/{id}"), title)
                    .content(json_str(node, "description"))
                    .author(json_str(node, "uploader.displayName"))
                    .image(urls[urls.len() - 1].clone())
                    .thumbnail(urls[0].clone())
                    .template(Template::Images)
                    .metadata("resolution", resolution(node))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn resolution(item: &serde_json::Value) -> String {
    let width = json_str(item, "width");
    let height = json_str(item, "height");
    if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width}x{height}")
    }
}

/// Standard base64 encoding (upstream uses `codecs.encode(..., "base64")`).
fn base64_encode(value: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": {"searchResource": {"edges": [
        {"node": {
          "id": "12345", "title": "Mountain lake", "description": "A lake",
          "width": 4000, "height": 3000,
          "urls": {"size_600": "https://img.500px.com/600.jpg",
                    "size_2048": "https://img.500px.com/2048.jpg"},
          "uploader": {"displayName": "Jane Doe"}
        }}
      ]}}
    }"#;

    fn engine() -> Arc<FiveHundredPx> {
        FiveHundredPx::from_config(&EngineConfig::new(
            "500px",
            "500px",
            "500px",
            &["stock images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_photo_results() {
        let response = EngineResponse::new(
            200,
            "https://api-neo.500px.com/graphql",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Mountain lake");
                assert_eq!(result.url, "https://500px.com/photo/12345");
                assert_eq!(result.author.as_deref(), Some("Jane Doe"));
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("4000x3000")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_graphql_cursor() {
        assert_eq!(base64_encode("pos-29"), "cG9zLTI5");
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.url, "https://api-neo.500px.com/graphql");
        let body = request.json.expect("json body");
        assert_eq!(body["variables"]["after"], "cG9zLTI5");
        assert_eq!(body["variables"]["first"], 30);
        assert_eq!(body["variables"]["keyword"], "lake");
    }
}
