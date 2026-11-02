//! Picjumbo engine, ported from searx/engines/picjumbo.py.
//!
//! Picjumbo offers free stock photos; the search page is parsed with the same
//! selectors as upstream and the full resolution URL is derived from the
//! thumbnail URL.
// engine-id: picjumbo

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
    shortcut: "pj",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://picjumbo.com";
const MAX_RES_QUERY: &str = "w=10000&quality=100";

/// Picjumbo stock photos.
pub struct Picjumbo {
    meta: EngineMeta,
}

impl Picjumbo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }
}

impl Engine for Picjumbo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/search/{}/page/{}",
            self.base_url(),
            encode(&query.query_text()),
            pageno.max(1)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.base_url().to_string();
        let mut results = Vec::new();
        let container = "div[class*='photo_query'] div[class*='photo_item']";
        for element in select_elements(&html, container)? {
            let Some(href) = attr_of(element, "a[class*='image']", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let Some(title) = text_of(element, "h3") else {
                continue;
            };
            let content =
                attr_of(element, "meta[itemprop='keywords']", "content").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .template(Template::Images);
            let thumbnail = attr_of(element, "img[class*='image']", "src").unwrap_or_default();
            if !thumbnail.is_empty() {
                if let Some(thumbnail) = absolute_url(&thumbnail, &base) {
                    if is_http_url(&thumbnail) {
                        let full = max_resolution_url(&thumbnail);
                        builder = builder.thumbnail(thumbnail).image(full);
                    }
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream asks imgix for the maximum resolution by replacing the query.
fn max_resolution_url(thumbnail: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(thumbnail) else {
        return thumbnail.to_string();
    };
    parsed.set_query(Some(MAX_RES_QUERY));
    parsed.to_string()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <div class="photo_query row">
      <div class="photo_item item">
        <a class="image" href="/free-photo/mountain/"><img class="image lazy" src="https://images.picjumbo.com/mountain.jpg?w=300"></a>
        <h3>Mountain lake</h3>
        <meta itemprop="keywords" content="mountain, lake, nature">
      </div>
      <div class="photo_item item">
        <a class="image" href="/free-photo/empty/"><img class="image" src="https://images.picjumbo.com/empty.jpg"></a>
        <h3></h3>
      </div>
    </div>"#;

    fn engine() -> Arc<Picjumbo> {
        Picjumbo::from_config(&EngineConfig::new(
            "picjumbo",
            "picjumbo",
            "pj",
            &["stock images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_images_and_builds_full_resolution_urls() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("mountain", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://picjumbo.com/free-photo/mountain/");
                assert_eq!(result.title, "Mountain lake");
                assert_eq!(result.content, "mountain, lake, nature");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://images.picjumbo.com/mountain.jpg?w=300")
                );
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://images.picjumbo.com/mountain.jpg?w=10000&quality=100")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_a_paged_search_url() {
        let query = Query::parse(
            "mountain lake",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://picjumbo.com/search/mountain%20lake/page/2"
        );
    }
}
