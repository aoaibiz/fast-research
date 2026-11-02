//! Imgur engine, ported from `searx/engines/imgur.py`.
//!
//! Upstream scrapes the HTML search page and needs no API key; when
//! `api_key` is configured it is sent as `Authorization: Client-ID ...`.
// engine-id: imgur

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
    shortcut: "img",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://imgur.com";

/// Imgur.
pub struct Imgur {
    meta: EngineMeta,
}

impl Imgur {
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

    fn api_key(&self) -> Option<String> {
        self.meta
            .api_key
            .clone()
            .or_else(|| self.meta.opt_str("api_key").map(str::to_string))
            .filter(|key| !key.trim().is_empty())
    }
}

impl Engine for Imgur {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let time_range = query
            .time_range
            .map(|range| range.as_str())
            .unwrap_or("all");
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("qs", "thumbs".to_string()),
            ("p", pageno.max(1).saturating_sub(1).to_string()),
        ];
        let mut request = EngineRequest::get(format!(
            "{}/search/score/{time_range}?{}",
            self.base_url(),
            query_string(&pairs)
        ));
        if let Some(api_key) = self.api_key() {
            request = request.header("Authorization", &format!("Client-ID {api_key}"));
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, "div.cards > div.post")? {
            let thumbnail_src = attr_of(element, "a img", "src").unwrap_or_default();
            // Bug on Imgur's side: sometimes there is no preview image.
            if thumbnail_src.chars().count() < 25 {
                continue;
            }
            let Some(title) = attr_of(element, "a img", "alt") else {
                continue;
            };
            let Some(href) = attr_of(element, "a", "href") else {
                continue;
            };
            let url = if href.starts_with("http://") || href.starts_with("https://") {
                href
            } else {
                format!("{base}{href}")
            };
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let img_src = thumbnail_src.replacen("b.", ".", 1);
            results.push(
                ResultBuilder::new(url, title)
                    .image(img_src)
                    .thumbnail(thumbnail_src)
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
    use crate::query::TimeRange;

    const FIXTURE: &str = r#"
    <html><body>
      <div class="cards">
        <div class="post">
          <a href="/gallery/abc123">
            <img src="https://i.imgur.com/abcd123b.jpg" alt="A funny image">
          </a>
        </div>
      </div>
    </body></html>"#;

    fn engine() -> Arc<Imgur> {
        Imgur::from_config(&EngineConfig::new("imgur", "imgur", "img", &["images"])).unwrap()
    }

    #[test]
    fn parses_post_cards() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("funny", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "A funny image");
                assert_eq!(result.url, "https://imgur.com/gallery/abc123");
                assert_eq!(result.template, Template::Images);
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://i.imgur.com/abcd123.jpg")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.imgur.com/abcd123b.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let mut query = Query::parse("funny", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://imgur.com/search/score/week?q=funny&qs=thumbs&p=1"
        );
    }
}
