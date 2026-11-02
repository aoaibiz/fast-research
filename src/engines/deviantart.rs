//! DeviantArt engine, ported from `searx/engines/deviantart.py`.
//!
//! The cursor based paging is exposed through engine data: the next cursor is
//! returned as `EngineItem::EngineData`, but `build_request` cannot read it
//! back, so only the first page is requested (upstream continues with the
//! stored cursor).
// engine-id: deviantart

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
    shortcut: "da",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const BASE_URL: &str = "https://www.deviantart.com";

/// DeviantArt.
pub struct Deviantart {
    meta: EngineMeta,
}

impl Deviantart {
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

impl Engine for Deviantart {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![("q", query.query_text())];
        let _ = pageno;
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for anchor in select_elements(&html, "div[data-testid=\"content_row\"] a")? {
            if child(anchor, "[data-testid=\"thumb\"]").is_none() {
                continue;
            }
            let thumbnail_src = attr_of(anchor, "img", "src").unwrap_or_default();
            let mut img_src = attr_of(anchor, "img", "srcset")
                .map(|srcset| srcset.split(' ').next().unwrap_or_default().to_string())
                .unwrap_or_default();
            if format!("{thumbnail_src}{img_src}").contains(",blur_") {
                continue;
            }
            if !img_src.is_empty() {
                img_src = strip_v1(&img_src);
            }
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            let title = attr(anchor, "aria-label").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let author =
                attr_of(anchor, "[property=\"schema:name\"]", "content").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .thumbnail(thumbnail_src)
                .author(author)
                .template(Template::Images);
            if !img_src.is_empty() {
                builder = builder.image(img_src);
            }
            results.push(builder.build());
        }
        if let Some(cursor) = next_cursor(&html, &base) {
            results.push(EngineItem::EngineData {
                key: "cursor".to_string(),
                value: cursor,
            });
        }
        Ok(results)
    }
}

/// Upstream drops everything behind `/v1` from the `srcset` URL.
fn strip_v1(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            let path = parsed.path().to_string();
            let trimmed = path.split("/v1").next().unwrap_or(&path).to_string();
            parsed.set_path(&trimmed);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// The `cursor` query parameter of the "Next" link.
fn next_cursor(html: &scraper::Html, base: &str) -> Option<String> {
    for anchor in select_elements(html, "a[href*='cursor=']").ok()? {
        if !text(anchor).contains("Next") {
            continue;
        }
        let href = attr(anchor, "href")?;
        let url = url::Url::parse(&href)
            .or_else(|_| url::Url::parse(base).and_then(|base| base.join(&href)))
            .ok()?;
        if let Some((_, value)) = url.query_pairs().find(|(key, _)| key == "cursor") {
            return Some(value.to_string());
        }
    }
    None
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <html><body>
      <div data-testid="content_row">
        <a href="https://www.deviantart.com/artist/art/My-Artwork-12345" aria-label="My Artwork">
          <div data-testid="thumb">
            <img src="https://images-wixmp-ed30a86b8c4ca887773594c2.wixmp.com/thumb.jpg"
                 srcset="https://images-wixmp-ed30a86b8c4ca887773594c2.wixmp.com/f/abc/v1/fill/w_100/art.jpg 1x, https://images-wixmp-ed30a86b8c4ca887773594c2.wixmp.com/f/abc/v1/fill/w_200/art.jpg 2x">
            <span property="schema:name" content="An Artist"></span>
          </div>
        </a>
      </div>
      <a href="/search?q=cats&amp;cursor=abc123">Next</a>
    </body></html>"#;

    fn engine() -> Arc<Deviantart> {
        Deviantart::from_config(&EngineConfig::new(
            "deviantart",
            "deviantart",
            "da",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_artwork() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("art", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "My Artwork");
                assert_eq!(
                    result.url,
                    "https://www.deviantart.com/artist/art/My-Artwork-12345"
                );
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://images-wixmp-ed30a86b8c4ca887773594c2.wixmp.com/f/abc")
                );
                assert_eq!(result.author.as_deref(), Some("An Artist"));
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::EngineData { key, value } => {
                assert_eq!(key, "cursor");
                assert_eq!(value, "abc123");
            }
            _ => panic!("expected engine data"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("art", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.url, "https://www.deviantart.com/search?q=art");
    }
}
