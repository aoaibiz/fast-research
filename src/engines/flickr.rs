//! Flickr engine, ported from searx/engines/flickr.py.
//!
//! The official API is used; an api_key has to be configured.
// engine-id: flickr

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
    shortcut: "fla",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.flickr.com/services/rest/";
const PHOTO_URL: &str = "https://www.flickr.com/photos";
/// Upstream: page_size = 15.
const PAGE_SIZE: i64 = 15;
/// The fields the response parser reads (upstream: extras=...).
const EXTRAS: &str = "description,owner_name,url_o,url_n,url_z";

/// Flickr, through the official API.
pub struct Flickr {
    meta: EngineMeta,
}

impl Flickr {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Flickr {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let api_key = self.meta.api_key.clone().unwrap_or_default();
        let page_size = self.meta.opt_i64("page_size").unwrap_or(PAGE_SIZE).max(1);
        let pairs: Vec<(&str, String)> = vec![
            ("method", "flickr.photos.search".to_string()),
            ("api_key", api_key),
            ("text", query.query_text()),
            ("sort", "relevance".to_string()),
            ("extras", EXTRAS.to_string()),
            ("per_page", page_size.to_string()),
            ("format", "json".to_string()),
            ("nojsoncallback", "1".to_string()),
            ("page", pageno.max(1).to_string()),
        ];
        let url = format!("{BASE_URL}?{}", query_string(&pairs));
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(photos) = json_path(&json, "photos.photo").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for photo in photos {
            let url_o = json_str(photo, "url_o");
            let url_z = json_str(photo, "url_z");
            let img_src = if !url_o.is_empty() {
                url_o
            } else if !url_z.is_empty() {
                url_z.clone()
            } else {
                continue;
            };
            // For a bigger thumbnail keep url_n, url_z is the fallback.
            let url_n = json_str(photo, "url_n");
            let thumbnail = if !url_n.is_empty() {
                url_n
            } else if !url_z.is_empty() {
                url_z
            } else {
                img_src.clone()
            };
            let id = json_str(photo, "id");
            let owner = json_str(photo, "owner");
            let title = json_str(photo, "title");
            if id.is_empty() || owner.is_empty() || title.is_empty() {
                continue;
            }
            results.push(
                ResultBuilder::new(format!("{PHOTO_URL}/{owner}/{id}"), title)
                    .content(json_str(photo, "description._content"))
                    .author(json_str(photo, "ownername"))
                    .image(img_src)
                    .thumbnail(thumbnail)
                    .template(Template::Images)
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"photos": {"photo": [
      {"id": "111", "owner": "222", "title": "Cat on a wall", "ownername": "Alice",
       "url_o": "https://live.staticflickr.com/1/o.jpg",
       "url_n": "https://live.staticflickr.com/1/n.jpg",
       "description": {"_content": "A cat."}},
      {"id": "333", "owner": "444", "title": "Small",
       "url_z": "https://live.staticflickr.com/3/z.jpg",
       "description": {"_content": ""}},
      {"id": "555", "owner": "666", "title": "No image"}
    ]}}"##;

    fn engine(api_key: Option<&str>) -> Arc<Flickr> {
        let mut config = EngineConfig::new("flickr", "flickr", "fla", &["stock images"]);
        config.api_key = api_key.map(|value| value.to_string());
        Flickr::from_config(&config).unwrap()
    }

    #[test]
    fn parses_photo_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine(None).parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.flickr.com/photos/222/111");
                assert_eq!(result.title, "Cat on a wall");
                assert_eq!(result.content, "A cat.");
                assert_eq!(result.author.as_deref(), Some("Alice"));
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://live.staticflickr.com/1/o.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://live.staticflickr.com/1/n.jpg")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an image result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Small");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://live.staticflickr.com/3/z.jpg")
                );
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn builds_the_api_request() {
        let query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine(Some("key123")).build_request(&query, 2).unwrap();
        assert!(request.url.contains("method=flickr.photos.search"));
        assert!(request.url.contains("api_key=key123"));
        assert!(request.url.contains("text=cat"));
        assert!(request.url.contains("per_page=15"));
        assert!(request.url.contains("page=2"));
    }
}
