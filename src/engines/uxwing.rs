//! UXwing engine, ported from searx/engines/uxwing.py.
// engine-id: uxwing

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "icons"],
    shortcut: "ux",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://uxwing.com";

/// UXwing, a free icon and image library.
pub struct Uxwing {
    meta: EngineMeta,
}

impl Uxwing {
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

impl Engine for Uxwing {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![("s", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{}/?{}",
            self.base_url(),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.base_url();
        let Ok(entries) = select_elements(&html, "article[id^='post']") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let Some(href) = attr_of(entry, "a", "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            let title = attr_of(entry, "img", "alt").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let classes = attr(entry, "class").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title)
                .content(class_tags(&classes).join(", "))
                .template(Template::Images);
            if let Some(src) = attr_of(entry, "img", "src") {
                if let Some(image) = absolute_url(&src, base) {
                    if is_http_url(&image) {
                        builder = builder.image(image);
                    }
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// Upstream turns the category and tag CSS classes into human readable tags.
fn class_tags(classes: &str) -> Vec<String> {
    let mut tags = Vec::new();
    for class in classes.split_whitespace() {
        for prefix in ["category", "tag"] {
            if let Some(rest) = class.strip_prefix(prefix) {
                let value = rest.replace('-', " ");
                if !value.is_empty() {
                    tags.push(title_case(&value));
                }
            }
        }
    }
    tags
}

/// Capitalise the first letter of every word of a tag.
fn title_case(value: &str) -> String {
    value
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
      <article id="post-12" class="categorylogos tagrust-programming">
        <a href="https://uxwing.com/rust-logo"><img src="https://cdn.uxwing.com/rust-logo.png"
          alt="Rust Logo"></a></article>
      <article id="post-13" class="tagbroken">
        <a href="javascript:void(0)"><img alt="Broken"></a></article>
    </body></html>"##;

    fn engine() -> Arc<Uxwing> {
        Uxwing::from_config(&EngineConfig::new("uxwing", "uxwing", "ux", &["images"])).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_icon_results_with_tags() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://uxwing.com/rust-logo");
        assert_eq!(result.title, "Rust Logo");
        assert_eq!(result.content, "Logos, Rust Programming");
        assert_eq!(result.template, Template::Images);
        assert_eq!(
            result.img_src.as_deref(),
            Some("https://cdn.uxwing.com/rust-logo.png")
        );
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query("rust logo"), 1).unwrap();
        assert_eq!(request.url, "https://uxwing.com/?s=rust%20logo");
    }

    #[test]
    fn titles_are_capitalised() {
        assert_eq!(title_case("rust programming"), "Rust Programming");
    }
}
