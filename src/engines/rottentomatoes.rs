//! RottenTomatoes engine, ported from searx/engines/rottentomatoes.py.

// engine-id: rottentomatoes

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["movies"],
    shortcut: "rt",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.rottentomatoes.com";

/// RottenTomatoes, the review aggregator.
pub struct RottenTomatoes {
    meta: EngineMeta,
}

impl RottenTomatoes {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for RottenTomatoes {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Ok(EngineRequest::get(format!(
            "{}/search?{}",
            self.base_url(),
            query_string(&[("search", query.query_text())])
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
        for row in select_elements(&html, "search-page-media-row")? {
            let Some(link) = child(row, "a") else {
                continue;
            };
            let (Some(href), Some(title)) = (attr(link, "href"), attr_of(row, "a img", "alt"))
            else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, &base) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let mut content = Vec::new();
            for (attribute, prefix) in [
                ("releaseyear", "From"),
                ("tomatometerscore", "Score:"),
                ("cast", "Starring"),
            ] {
                if let Some(value) = attr(row, attribute) {
                    content.push(format!("{prefix} {value}"));
                }
            }
            let mut builder = ResultBuilder::new(url, title).content(content.join(", "));
            if let Some(src) = attr_of(row, "a img", "src") {
                if let Some(thumbnail) = absolute_url(&src, &base) {
                    if is_http_url(&thumbnail) {
                        builder = builder.thumbnail(thumbnail);
                    }
                }
            }
            results.push(builder.build());
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

    const FIXTURE: &str = r##"<html><body>
<search-page-media-row releaseyear="2024" tomatometerscore="88" cast="Jane Doe, John Roe">
  <a href="/m/first_movie"><img src="https://example.test/one.jpg" alt="First movie"></a>
</search-page-media-row>
<search-page-media-row releaseyear="1999" tomatometerscore="">
  <a href="https://www.rottentomatoes.com/m/second_movie"><img alt="Second movie"></a>
</search-page-media-row>
<search-page-media-row releaseyear="2001">
  <a href="/m/nameless"><img src="/none.jpg"></a>
</search-page-media-row>
</body></html>"##;

    fn engine() -> Arc<RottenTomatoes> {
        RottenTomatoes::from_config(&EngineConfig::new(
            "rottentomatoes",
            "rottentomatoes",
            "rt",
            &["movies"],
        ))
        .unwrap()
    }

    fn query() -> Query {
        Query::parse("dune", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_movie_rows() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "First movie");
                assert_eq!(result.url, "https://www.rottentomatoes.com/m/first_movie");
                assert_eq!(
                    result.content,
                    "From 2024, Score: 88, Starring Jane Doe, John Roe"
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://example.test/one.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert_eq!(result.content, "From 1999");
                assert!(result.thumbnail.is_none());
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query(), 1).unwrap();
        assert_eq!(
            request.url,
            "https://www.rottentomatoes.com/search?search=dune"
        );
    }

    #[test]
    fn missing_rows_yield_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
