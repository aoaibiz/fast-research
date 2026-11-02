//! Art Institute of Chicago engine, ported from `searx/engines/artic.py`.

// engine-id: artic

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "arc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://api.artic.edu/api/v1/artworks/search";
const IMAGE_API: &str = "https://www.artic.edu/iiif/2/";
const PAGE_SIZE: u32 = 20;

/// The Art Institute of Chicago.
pub struct Artic {
    meta: EngineMeta,
}

impl Artic {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Artic {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.to_string()),
            (
                "fields",
                "id,title,artist_display,medium_display,image_id,date_display,dimensions,artist_titles"
                    .to_string(),
            ),
            ("limit", PAGE_SIZE.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{SEARCH_API}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "data").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let image_id = json_str(item, "image_id");
            if image_id.is_empty() {
                continue;
            }
            let id = json_str(item, "id");
            let title = {
                let date = json_str(item, "date_display");
                let artist = json_str(item, "artist_display");
                let mut text = json_str(item, "title");
                if !date.is_empty() {
                    text = format!("{text} ({date})");
                }
                if !artist.is_empty() {
                    text = format!("{text} // {artist}");
                }
                text
            };
            if title.is_empty() {
                continue;
            }
            let content = {
                let medium = json_str(item, "medium_display");
                let dimensions = json_str(item, "dimensions");
                if dimensions.is_empty() {
                    medium
                } else if medium.is_empty() {
                    dimensions
                } else {
                    format!("{medium} // {dimensions}")
                }
            };
            let url = format!("https://artic.edu/artworks/{id}");
            let img_src = format!("{IMAGE_API}{image_id}/full/843,/0/default.jpg");
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .image(img_src)
                .template(Template::Images);
            let authors: Vec<String> = json_path(item, "artist_titles")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !authors.is_empty() {
                builder = builder.author(authors.join(", "));
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_artwork_json() {
        let body = r#"{"data":[
          {"id":78674,"title":"The Bedroom","date_display":"1889","artist_display":"Vincent van Gogh",
           "medium_display":"Oil on canvas","dimensions":"73.7 × 92.1 cm",
           "image_id":"abc123","artist_titles":["Vincent van Gogh"]}
        ]}"#;
        let response = EngineResponse::new(200, SEARCH_API, body.as_bytes().to_vec());
        let engine =
            Artic::from_config(&EngineConfig::new("artic", "artic", "arc", &["images"])).unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("van gogh", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://artic.edu/artworks/78674");
                assert_eq!(result.title, "The Bedroom (1889) // Vincent van Gogh");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://www.artic.edu/iiif/2/abc123/full/843,/0/default.jpg")
                );
            }
            _ => panic!("expected an image result"),
        }
    }
}
