//! Pinterest engine, ported from searx/engines/pinterest.py.
//! The search resource is queried with the JSON data parameter of the web UI.
// engine-id: pinterest

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
    shortcut: "pin",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.pinterest.com";

pub struct Pinterest {
    meta: EngineMeta,
}

impl Pinterest {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The base URL without its trailing slash.
    fn base(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Pinterest {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let bookmark = self.meta.opt_str("bookmark").unwrap_or_default();
        let text = query.query_text();
        let args =
            serde_json::json!({"options": {"query": text, "bookmarks": [bookmark]}, "context": {}});
        let data = serde_json::to_string(&args).unwrap_or_default();
        let url = format!(
            "{}/resource/BaseSearchResource/get/?data={}",
            self.base(),
            encode(&data)
        );
        Ok(EngineRequest::get(url)
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-Pinterest-AppState", "active")
            .header("X-Pinterest-Source-Url", "/ideas/")
            .header("X-Pinterest-PWS-Handler", "www/ideas.js"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(results) =
            json_path(&json, "resource_response.data.results").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let base = self.base();
        let mut items = Vec::new();
        for result in results {
            if json_str(result, "type") == "story" {
                continue;
            }
            let Some(images) = json_path(result, "images") else {
                continue;
            };
            let img_src = json_str(images, "orig.url");
            if img_src.is_empty() {
                continue;
            }
            let title = pin_title(result);
            if title.is_empty() {
                continue;
            }
            let id = json_str(result, "id");
            let link = json_str(result, "link");
            let url = if link.starts_with("http") {
                link
            } else if id.is_empty() {
                continue;
            } else {
                format!("{base}/pin/{id}/")
            };
            let width = json_str(images, "orig.width");
            let height = json_str(images, "orig.height");
            let username = json_str(result, "pinner.username");
            let full_name = json_str(result, "pinner.full_name");
            let author = if username.is_empty() {
                full_name
            } else {
                format!("{full_name} ({username})")
            };
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(result, "rich_summary.display_description"))
                .image(img_src)
                .template(Template::Images)
                .author(author);
            let thumbnail = json_str(images, "236x.url");
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
            }
            if !width.is_empty() && !height.is_empty() {
                builder = builder.metadata("resolution", format!("{width}x{height}"));
            }
            let source = json_str(result, "rich_summary.site_name");
            if !source.is_empty() {
                builder = builder.metadata("source", source);
            }
            items.push(builder.build());
        }
        // Paging continues with the bookmark; build_request cannot read engine
        // data back, so only a configured bookmark can be requested again.
        let bookmark = json_str(&json, "resource_response.bookmark");
        if !bookmark.is_empty() {
            items.push(EngineItem::EngineData {
                key: "bookmark".to_string(),
                value: bookmark,
            });
        }
        Ok(items)
    }
}

/// Upstream's title fallback chain: title, grid_title, visual annotation, name.
fn pin_title(result: &serde_json::Value) -> String {
    let mut title = json_str(result, "title");
    if title.is_empty() {
        title = json_str(result, "grid_title");
    }
    if title.chars().count() < 5 {
        let annotation = json_str(result, "pin_join.visual_annotation.0");
        let name = json_str(result, "name");
        let fallback = if name.is_empty() {
            json_str(result, "auto_alt_text")
        } else {
            name
        };
        title = if annotation.is_empty() {
            fallback
        } else {
            annotation
        };
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"resource_response": {"bookmark": "next-page-token",
      "data": {"results": [
        {"type": "pin", "id": "123", "title": "Cozy reading corner",
         "link": "https://example.com/pin-page",
         "rich_summary": {"display_description": "A reading corner", "site_name": "Example"},
         "images": {"orig": {"url": "https://i.pinimg.com/orig.jpg", "width": 1200, "height": 800},
                    "236x": {"url": "https://i.pinimg.com/236x.jpg"}},
         "pinner": {"full_name": "Jane Doe", "username": "janedoe"}},
        {"type": "story", "id": "999", "title": "A story"},
        {"type": "pin", "id": "456", "grid_title": "Moody forest photo",
         "images": {"orig": {"url": "https://i.pinimg.com/forest.jpg", "width": 600, "height": 900},
                    "236x": {"url": "https://i.pinimg.com/forest-236x.jpg"}}}
      ]}}}"##;

    fn engine() -> Arc<Pinterest> {
        Pinterest::from_config(&EngineConfig::new(
            "pinterest",
            "pinterest",
            "pin",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_pin_results_and_the_bookmark() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cozy", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 3);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/pin-page");
                assert_eq!(result.title, "Cozy reading corner");
                assert_eq!(result.content, "A reading corner");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://i.pinimg.com/orig.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.pinimg.com/236x.jpg")
                );
                assert_eq!(result.author.as_deref(), Some("Jane Doe (janedoe)"));
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("1200x800")
                );
                assert_eq!(
                    result.metadata.get("source").map(String::as_str),
                    Some("Example")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an image result"),
        }
        let Some(EngineItem::Result(second)) = items.get(1) else {
            panic!("expected a second result");
        };
        assert_eq!(second.url, "https://www.pinterest.com/pin/456/");
        assert_eq!(second.title, "Moody forest photo");
        let Some(EngineItem::EngineData { key, value }) = items.get(2) else {
            panic!("expected engine data");
        };
        assert_eq!(key, "bookmark");
        assert_eq!(value, "next-page-token");
    }

    #[test]
    fn builds_the_json_data_request() {
        let query = Query::parse("cozy", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        let prefix = "https://www.pinterest.com/resource/BaseSearchResource/get/?data=";
        assert!(request.url.starts_with(prefix));
        let encoded = request.url.trim_start_matches(prefix);
        let data = percent_encoding::percent_decode_str(encoded)
            .decode_utf8()
            .unwrap();
        let args: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(args["options"]["query"], "cozy");
        assert_eq!(args["options"]["bookmarks"][0], "");
    }
}
