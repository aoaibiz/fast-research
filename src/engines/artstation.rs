//! Artstation engine, ported from `searx/engines/artstation.py`.
//!
//! The API is protected by a double submit CSRF token pair.  Upstream fetches
//! the pair in `request()`; here it is read from the configuration
//! (`public_csrf_token`, `private_csrf_token`) so the request stays
//! synchronous.  Without the pair the API usually answers 403.
// engine-id: artstation

use std::sync::Arc;

use regex::Regex;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "as",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.artstation.com/api/v2/search/projects.json";
const RESULTS_PER_PAGE: u32 = 20;

/// Artstation.
pub struct Artstation {
    meta: EngineMeta,
}

impl Artstation {
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
    }
}

impl Engine for Artstation {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let body = serde_json::json!({
            "query": query.query_text(),
            "page": pageno.max(1),
            "per_page": RESULTS_PER_PAGE,
            "sorting": "relevance",
            "pro_first": 1,
        });
        let mut request = EngineRequest::post(self.base_url())
            .header("content-type", "application/json")
            .header("Accept", "application/json")
            .json_body(body);
        if let Some(token) = self.meta.opt_str("public_csrf_token") {
            request = request.header("PUBLIC-CSRF-TOKEN", token);
        }
        if let Some(token) = self.meta.opt_str("private_csrf_token") {
            request = request.cookie("PRIVATE-CSRF-TOKEN", token);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "data").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let rewrite = Regex::new(r"/\d{6,}/").ok();
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "title");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let thumbnail = json_str(item, "smaller_square_cover_url");
            let img_src = match &rewrite {
                Some(regex) => regex.replace_all(&thumbnail, "/").to_string(),
                None => thumbnail.clone(),
            }
            .replace("smaller_square", "large");
            let author = {
                let username = json_str(item, "user.username");
                let full_name = json_str(item, "user.full_name");
                if full_name.is_empty() {
                    username
                } else {
                    format!("{username} ({full_name})")
                }
            };
            results.push(
                ResultBuilder::new(url, title)
                    .author(author)
                    .image(img_src)
                    .thumbnail(thumbnail)
                    .template(Template::Images)
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "data": [
        {
          "title": "A painting",
          "url": "https://www.artstation.com/artwork/abc123",
          "smaller_square_cover_url": "https://cdna.artstation.com/p/assets/images/images/012/345/smaller_square/artist-painting.jpg",
          "user": {"username": "artist", "full_name": "An Artist"}
        }
      ]
    }"#;

    fn engine() -> Arc<Artstation> {
        Artstation::from_config(&EngineConfig::new(
            "artstation",
            "artstation",
            "as",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_projects() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("painting", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A painting");
                assert_eq!(result.url, "https://www.artstation.com/artwork/abc123");
                assert_eq!(result.author.as_deref(), Some("artist (An Artist)"));
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some(
                        "https://cdna.artstation.com/p/assets/images/images/012/345/large/artist-painting.jpg"
                    )
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_post_request() {
        let query = Query::parse("painting", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, BASE_URL);
        let body = request.json.expect("a JSON body");
        assert_eq!(body["query"], "painting");
        assert_eq!(body["page"], 2);
        assert_eq!(body["per_page"], 20);
    }
}
