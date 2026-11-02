//! Magnific engine, ported from searx/engines/magnific.py.
//!
//! Upstream solves the bot protection challenge with a second request and a
//! cached cookie. This port sends the search request only and reports the
//! challenge page as a captcha error.

// engine-id: magnific

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
    shortcut: "mag",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.magnific.com";

/// Magnific stock image search.
pub struct Magnific {
    meta: EngineMeta,
}

impl Magnific {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

impl Engine for Magnific {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = base_url(&self.meta);
        let free_images_only = self.meta.opt_bool("free_images_only").unwrap_or(true);
        let mut pairs: Vec<(&str, String)> = vec![
            ("term", query.query_text()),
            ("filters[ai-generated][excluded]", "1".to_string()),
            ("page", pageno.max(1).to_string()),
            ("locale", "en".to_string()),
        ];
        if free_images_only {
            pairs.push(("filters[license]", "free".to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{base}/api/regular/search?{}",
            query_string(&pairs)
        ))
        .header("Referer", &format!("{base}/search"))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // The challenge page is HTML, the API answers with JSON.
        if let Some(content_type) = response.header("content-type") {
            if content_type.to_ascii_lowercase().contains("text/html") {
                return Err(EngineError::Captcha);
            }
        }
        let json = response.json()?;
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "name");
            let url = json_str(item, "url");
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title).template(Template::Images);
            if let Some(preview) = json_path(item, "preview") {
                let preview_url = json_str(preview, "url");
                if is_http(&preview_url) {
                    builder = builder.image(preview_url.clone()).thumbnail(preview_url);
                }
                let width = json_str(preview, "width");
                let height = json_str(preview, "height");
                if !width.is_empty() && !height.is_empty() {
                    builder = builder.metadata("resolution", format!("{width}x{height}"));
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "items": [
        {"name": "Sunset", "url": "https://www.magnific.com/photo/sunset",
         "preview": {"url": "https://cdn.magnific.com/sunset.jpg", "width": 800, "height": 600}},
        {"name": "", "url": "https://www.magnific.com/photo/empty", "preview": {"url": ""}},
        {"name": "Bad", "url": "javascript:alert(1)"}
      ]
    }"##;

    fn engine() -> Arc<Magnific> {
        Magnific::from_config(&EngineConfig::new(
            "magnific",
            "magnific",
            "mag",
            &["stock images"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_image_results() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("sunset")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Sunset");
                assert_eq!(result.url, "https://www.magnific.com/photo/sunset");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://cdn.magnific.com/sunset.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://cdn.magnific.com/sunset.jpg")
                );
                assert_eq!(
                    result.metadata.get("resolution").map(String::as_str),
                    Some("800x600")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("red car"), 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.magnific.com/api/regular/search?")
        );
        assert!(request.url.contains("term=red%20car"));
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("filters%5Blicense%5D=free"));
        let referer = request
            .headers
            .get("referer")
            .map(|v| v.to_str().unwrap_or_default());
        assert_eq!(referer, Some("https://www.magnific.com/search"));
    }

    #[test]
    fn challenge_pages_are_captchas() {
        let mut response = EngineResponse::new(200, BASE_URL, b"<html>verify</html>".to_vec());
        response.headers.insert(
            "content-type".to_string(),
            "text/html; charset=utf-8".to_string(),
        );
        assert!(matches!(
            engine().parse(&response, &query("sunset")),
            Err(EngineError::Captcha)
        ));
    }
}
