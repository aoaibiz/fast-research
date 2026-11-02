//! FindThatMeme engine, ported from searx/engines/findthatmeme.py.

// engine-id: findthatmeme

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
    shortcut: "ftm",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://findthatmeme.com/api/v1/search";
const MEDIA_URL: &str = "https://s3.thehackerblog.com/findthatmeme";
const PAGE_SIZE: u32 = 50;

/// FindThatMeme, a meme reverse image search.
pub struct FindThatMeme {
    meta: EngineMeta,
}

impl FindThatMeme {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for FindThatMeme {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let offset = (pageno.max(1) - 1) * PAGE_SIZE;
        let body = serde_json::json!({
            "search": query.query_text(),
            "offset": offset,
        });
        Ok(EngineRequest::post(base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json.as_array() {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "source_page_url");
            let title = json_str(item, "source_site");
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let image_type = json_str(item, "type");
            let path = if image_type == "IMAGE" {
                json_str(item, "image_path")
            } else {
                json_str(item, "thumbnail")
            };
            let image_url = if image_type == "IMAGE" {
                format!("{MEDIA_URL}/{path}")
            } else {
                format!("{MEDIA_URL}/thumb/{path}")
            };
            let mut builder = ResultBuilder::new(url, title).template(Template::Images);
            if !path.is_empty() {
                builder = builder.image(image_url);
            }
            if let Some(size) = json_path(item, "meme_file_size").and_then(|value| value.as_f64()) {
                builder = builder.metadata("filesize", humanize_bytes(size));
            }
            let updated = json_str(item, "updated_at");
            let date = updated.split('T').next().unwrap_or_default();
            results.push(builder.published_date(date).build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The 1024 based byte formatting of upstream's humanize_bytes helper.
fn humanize_bytes(size: f64) -> String {
    const UNITS: [&str; 5] = ["B ", "KB", "MB", "GB", "TB"];
    let mut value = size;
    let mut index = 0;
    while value > 1024.0 && index + 1 < UNITS.len() {
        index += 1;
        value /= 1024.0;
    }
    format!("{value:.2} {}", UNITS[index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"[
      {"image_path":"abc.jpg","thumbnail":"abc-thumb.jpg","type":"IMAGE",
       "source_page_url":"https://example.com/post","source_site":"Example",
       "meme_file_size":2048,"updated_at":"2024-05-06T10:00:00Z"},
      {"image_path":"clip.mp4","thumbnail":"clip.jpg","type":"VIDEO",
       "source_page_url":"https://example.com/video","source_site":"Video Site",
       "meme_file_size":1572864,"updated_at":"2024-06-01T00:00:00Z"},
      {"image_path":"broken.jpg","type":"IMAGE","source_site":"No URL"}
    ]"##;

    fn engine() -> Arc<FindThatMeme> {
        FindThatMeme::from_config(&EngineConfig::new(
            "findthatmeme",
            "findthatmeme",
            "ftm",
            &["images"],
        ))
        .unwrap()
    }

    fn search() -> Query {
        Query::parse("cat", &EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&search(), 2).unwrap();
        assert_eq!(request.url, BASE_URL);
        let body = request.json.as_ref().unwrap();
        assert_eq!(json_str(body, "search"), "cat");
        assert_eq!(json_str(body, "offset"), "50");
    }

    #[test]
    fn parses_memes() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &search()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/post");
                assert_eq!(result.title, "Example");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://s3.thehackerblog.com/findthatmeme/abc.jpg")
                );
                assert_eq!(result.published_date.as_deref(), Some("2024-05-06"));
                assert_eq!(
                    result.metadata.get("filesize").map(String::as_str),
                    Some("2.00 KB")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://s3.thehackerblog.com/findthatmeme/thumb/clip.jpg")
                );
                assert_eq!(
                    result.metadata.get("filesize").map(String::as_str),
                    Some("1.50 MB")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn missing_container_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{\"error\":\"nope\"}".to_vec());
        assert!(engine().parse(&response, &search()).unwrap().is_empty());
    }
}
