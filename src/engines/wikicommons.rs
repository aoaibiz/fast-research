//! Wikimedia Commons engine, ported from `searx/engines/wikicommons.py`
//! (the image search type; the file/video/audio variants are not ported).

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
    shortcut: "wcm",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const API_URL: &str = "https://commons.wikimedia.org/w/api.php";
const PAGE_SIZE: u32 = 10;

/// Wikimedia Commons image search through the MediaWiki query API.
pub struct Wikicommons {
    meta: EngineMeta,
}

impl Wikicommons {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Wikicommons {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("action", "query".to_string()),
            ("generator", "search".to_string()),
            ("gsrsearch", query.query_text()),
            ("gsrnamespace", "6".to_string()),
            ("gsrlimit", PAGE_SIZE.to_string()),
            ("gsroffset", ((pageno.max(1) - 1) * PAGE_SIZE).to_string()),
            ("prop", "imageinfo".to_string()),
            ("iiprop", "url|extmetadata".to_string()),
            ("iiurlwidth", "400".to_string()),
            ("format", "json".to_string()),
        ];
        let language = locale_language(&query.locale);
        if language != "all" {
            pairs.push(("uselang", language));
        }
        Ok(
            EngineRequest::get(format!("{API_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(pages) = json_path(&json, "query.pages").and_then(serde_json::Value::as_object)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in pages.values() {
            let Some(imageinfo) = json_path(item, "imageinfo.0") else {
                continue;
            };
            let title = page_title(item);
            let url = json_str(imageinfo, "descriptionurl");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let image = json_str(imageinfo, "url");
            let thumbnail = if json_str(imageinfo, "thumburl").is_empty() {
                image.clone()
            } else {
                json_str(imageinfo, "thumburl")
            };
            results.push(
                ResultBuilder::new(url, title)
                    .content(extmetadata(imageinfo, "ImageDescription"))
                    .image(image)
                    .thumbnail(thumbnail)
                    .author(extmetadata(imageinfo, "Artist"))
                    .published_date(extmetadata(imageinfo, "DateTimeOriginal"))
                    .template(Template::Images)
                    .metadata("resolution", resolution(imageinfo))
                    .metadata("mime", json_str(imageinfo, "mime"))
                    .metadata("filesize", json_str(imageinfo, "size"))
                    .metadata("license", extmetadata(imageinfo, "LicenseShortName"))
                    .metadata("page_id", json_str(item, "pageid"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The file name without the File: prefix and without the extension.
fn page_title(item: &serde_json::Value) -> String {
    let raw = json_str(item, "title");
    let raw = raw.strip_prefix("File:").unwrap_or(&raw);
    raw.rsplit_once('.')
        .map(|(name, _)| name)
        .unwrap_or(raw)
        .to_string()
}

/// The pixel size, like upstream's resolution field.
fn resolution(imageinfo: &serde_json::Value) -> String {
    let width = json_str(imageinfo, "width");
    let height = json_str(imageinfo, "height");
    if width.is_empty() || height.is_empty() {
        String::new()
    } else {
        format!("{width} x {height}")
    }
}

/// One extmetadata value; the API stores them as HTML fragments.
fn extmetadata(imageinfo: &serde_json::Value, key: &str) -> String {
    let value = imageinfo
        .get("extmetadata")
        .and_then(|metadata| metadata.get(key))
        .and_then(|entry| entry.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    strip_tags(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "batchcomplete": "",
      "query": {
        "pages": {
          "12345": {
            "pageid": 12345,
            "ns": 6,
            "title": "File:Sunset over the sea.jpg",
            "imageinfo": [
              {
                "timestamp": 1700000000,
                "user": "JaneDoe",
                "url": "https://upload.wikimedia.org/wikipedia/commons/a/ab/Sunset_over_the_sea.jpg",
                "descriptionurl": "https://commons.wikimedia.org/wiki/File:Sunset_over_the_sea.jpg",
                "width": 1024,
                "height": 768,
                "size": 204800,
                "mime": "image/jpeg",
                "thumburl": "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Sunset_over_the_sea.jpg/400px-Sunset_over_the_sea.jpg",
                "extmetadata": {
                  "ImageDescription": {"value": "<p>A sunset.</p>", "source": "commons-db"},
                  "Artist": {"value": "<a href=\"/wiki/User:JaneDoe\">Jane Doe</a>", "source": "commons-db"},
                  "LicenseShortName": {"value": "CC BY-SA 4.0", "source": "commons-db"},
                  "DateTimeOriginal": {"value": "2024-01-01", "source": "commons-db"}
                }
              }
            ]
          }
        }
      }
    }"#;

    fn engine() -> Arc<Wikicommons> {
        Wikicommons::from_config(&EngineConfig::new(
            "wikicommons.images",
            "wikicommons",
            "wcm",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_imageinfo_pages() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("sunset", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Sunset over the sea");
                assert_eq!(
                    result.url,
                    "https://commons.wikimedia.org/wiki/File:Sunset_over_the_sea.jpg"
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some(
                        "https://upload.wikimedia.org/wikipedia/commons/a/ab/Sunset_over_the_sea.jpg"
                    )
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some(
                        "https://upload.wikimedia.org/wikipedia/commons/thumb/a/ab/Sunset_over_the_sea.jpg/400px-Sunset_over_the_sea.jpg"
                    )
                );
                assert_eq!(result.author.as_deref(), Some("Jane Doe"));
                assert_eq!(result.content, "A sunset.");
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("1024 x 768")
                );
                assert_eq!(
                    result.metadata.get("license").map(String::as_str),
                    Some("CC BY-SA 4.0")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_mediawiki_url() {
        let query = Query::parse("sunset", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(request.url.starts_with(API_URL));
        assert!(request.url.contains("gsrnamespace=6"));
        assert!(request.url.contains("gsrlimit=10"));
        assert!(request.url.contains("gsroffset=20"));
        assert!(request.url.contains("iiprop=url%7Cextmetadata"));
        assert!(request.url.contains("iiurlwidth=400"));
    }
}
