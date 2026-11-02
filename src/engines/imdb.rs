//! IMDb engine, ported from `searx/engines/imdb.py`.
// engine-id: imdb

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["movies"],
    shortcut: "imdb",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const SUGGESTION_URL: &str = "https://v2.sg.media-imdb.com/suggestion/{letter}/{query}.json";
const HREF_BASE: &str = "https://imdb.com/{category}/{entry_id}";

/// IMDb.
pub struct Imdb {
    meta: EngineMeta,
}

impl Imdb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Imdb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text().replace(' ', "_").to_lowercase();
        let Some(letter) = text.chars().next() else {
            return Err(EngineError::Message("imdb: empty query".into()));
        };
        Ok(EngineRequest::get(
            SUGGESTION_URL
                .replace("{letter}", &letter.to_string())
                .replace("{query}", &encode(&text)),
        )
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "d").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let entry_id = json_str(entry, "id");
            let title = json_str(entry, "l");
            if entry_id.is_empty() || title.is_empty() {
                continue;
            }
            let Some(category) = search_category(&entry_id) else {
                continue;
            };
            let mut title = title;
            let qualifier = json_str(entry, "q");
            if !qualifier.is_empty() {
                title = format!("{title} ({qualifier})");
            }
            let mut content = String::new();
            let rank = json_str(entry, "rank");
            if !rank.is_empty() {
                content.push_str(&format!("({rank}) "));
            }
            let year = json_str(entry, "y");
            if !year.is_empty() {
                content.push_str(&format!("{year} - "));
            }
            content.push_str(&json_str(entry, "s"));
            let thumbnail = thumbnail(json_str(entry, "i.imageUrl"));
            results.push(
                ResultBuilder::new(
                    HREF_BASE
                        .replace("{category}", category)
                        .replace("{entry_id}", &entry_id),
                    title,
                )
                .content(content)
                .thumbnail(thumbnail)
                .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream `search_categories`, keyed by the IMDb id prefix.
fn search_category(entry_id: &str) -> Option<&'static str> {
    match entry_id.get(..2)? {
        "nm" => Some("name"),
        "tt" => Some("title"),
        "kw" => Some("keyword"),
        "co" => Some("company"),
        "ep" => Some("episode"),
        _ => None,
    }
}

/// Upstream's magic thumbnail recipe.
fn thumbnail(image_url: String) -> String {
    if image_url.is_empty() {
        return String::new();
    }
    let Some((name, prefix)) = image_url.rsplit_once('.') else {
        return image_url;
    };
    let magic = "QL75_UX280_CR0,0,280,414_";
    if name.ends_with("_V1_") {
        format!("{name}{magic}.{prefix}")
    } else {
        format!("{name}_V1_{magic}.{prefix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "d": [
        {
          "id": "tt0133093",
          "l": "The Matrix",
          "y": 1999,
          "rank": 20,
          "s": "Keanu Reeves, Laurence Fishburne",
          "i": {"imageUrl": "https://m.media-amazon.com/images/M/abc._V1_.jpg"}
        }
      ]
    }"#;

    fn engine() -> Arc<Imdb> {
        Imdb::from_config(&EngineConfig::new("imdb", "imdb", "imdb", &["movies"])).unwrap()
    }

    #[test]
    fn parses_suggestions() {
        let response = EngineResponse::new(200, SUGGESTION_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("the matrix", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "The Matrix");
                assert_eq!(result.url, "https://imdb.com/title/tt0133093");
                assert_eq!(
                    result.content,
                    "(20) 1999 - Keanu Reeves, Laurence Fishburne"
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some(
                        "https://m.media-amazon.com/images/M/abc._V1_QL75_UX280_CR0,0,280,414_.jpg"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_suggestion_url() {
        let query = Query::parse("The Matrix", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://v2.sg.media-imdb.com/suggestion/t/the_matrix.json"
        );
    }
}
