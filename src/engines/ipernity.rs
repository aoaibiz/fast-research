//! Ipernity engine, ported from searx/engines/ipernity.py.
// engine-id: ipernity

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
    shortcut: "ip",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.ipernity.com";
const PAGE_SIZE: i64 = 10;

/// Ipernity, a photo sharing community.
pub struct Ipernity {
    meta: EngineMeta,
}

impl Ipernity {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for Ipernity {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/search/photo/@/page:{}:{}?q={}",
            self.base_url(),
            pageno,
            PAGE_SIZE,
            quote_plus(&query.query_text())
        )))
    }

    /// Upstream cuts the JSON object out of the inline scripts.
    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let images: Vec<String> = select_elements(&html, "a[href^='/doc'] img")?
            .into_iter()
            .map(|image| attr(image, "src").unwrap_or_default())
            .collect();
        let mut results = Vec::new();
        let mut index = 0;
        for script in select_elements(&html, "script[type='text/javascript']")? {
            let script_text: String = script.text().collect();
            let Some(body) = between(&script_text, "] = ", "};") else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&format!("{body}}}")) else {
                continue;
            };
            if json_str(&json, "mediakey").is_empty() {
                continue;
            }
            let Some(thumbnail) = images.get(index).cloned() else {
                break;
            };
            let user_id = json_str(&json, "user_id");
            let doc_id = json_str(&json, "doc_id");
            let title = json_str(&json, "title");
            if user_id.is_empty() || doc_id.is_empty() || title.is_empty() {
                continue;
            }
            let url = format!("{base}/doc/{user_id}/{doc_id}");
            if !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(strip_tags(&json_str(&json, "content")))
                .template(Template::Images)
                .published_date(published_date(&json_str(&json, "posted_at")));
            if !thumbnail.is_empty() {
                builder = builder
                    .thumbnail(thumbnail.clone())
                    .image(thumbnail.replace("240.jpg", "640.jpg"));
            }
            if let Some(resolution) = resolution(&json) {
                builder = builder.metadata("resolution", resolution);
            }
            results.push(builder.build());
            index += 1;
        }
        Ok(results)
    }
}

/// Upstream's extr: the text between two markers, without either of them.
fn between<'a>(text: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = text.find(begin)? + begin.len();
    let rest = &text[start..];
    Some(&rest[..rest.find(end)?])
}

/// Python's quote_plus: spaces become plus signs.
fn quote_plus(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        match byte {
            b' ' => output.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(*byte as char)
            }
            other => output.push_str(&format!("%{other:02X}")),
        }
    }
    output
}

fn resolution(json: &serde_json::Value) -> Option<String> {
    let (width, height) = (json_str(json, "width"), json_str(json, "height"));
    (!width.is_empty() && !height.is_empty()).then(|| format!("{width}x{height}"))
}

fn published_date(value: &str) -> String {
    let Ok(timestamp) = value.trim().parse::<i64>() else {
        return String::new();
    };
    match chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, 0) {
        Some(date) => date.format("%Y-%m-%dT%H:%M:%S").to_string(),
        None => String::new(),
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Ipernity> {
        Ipernity::from_config(&EngineConfig::new(
            "ipernity",
            "ipernity",
            "ip",
            &["images"],
        ))
        .unwrap()
    }

    const FIXTURE: &str = r##"
    <a href="/doc/42/99"><img src="https://img.example.com/abc_240.jpg"></a>
    <script type="text/javascript">
      album[0] = {"mediakey":"abc","user_id":"42","doc_id":"99","title":"A photo","content":"A description","width":240,"height":180,"posted_at":"1700000000"};
    </script>
    <script type="text/javascript">album[1] = {"other":"value"};</script>"##;

    #[test]
    fn parses_the_embedded_photo_metadata() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("photo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ipernity.com/doc/42/99");
                assert_eq!(result.title, "A photo");
                assert_eq!(result.content, "A description");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/abc_240.jpg")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.example.com/abc_640.jpg")
                );
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("240x180")
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2023-11-14T22:13:20")
                );
            }
            _ => panic!("expected a picture"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("rust photo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.ipernity.com/search/photo/@/page:2:10?q=rust+photo"
        );
    }
}
