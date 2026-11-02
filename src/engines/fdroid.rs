//! F-Droid engine, ported from searx/engines/fdroid.py.
//!
//! F-Droid is a repository of free and open source Android applications. The
//! search site is scraped, like upstream; the summary and the licence of each
//! package make up the result content.
// engine-id: fdroid

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "apps"],
    shortcut: "fd",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://search.f-droid.org";

/// F-Droid application search.
pub struct Fdroid {
    meta: EngineMeta,
}

impl Fdroid {
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

impl Engine for Fdroid {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("lang", String::new()),
        ];
        let url = format!("{}/?{}", self.base_url(), query_string(&pairs));
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let base = self.base_url();
        let mut results = Vec::new();
        for element in select_elements(&html, "a.package-header")? {
            let title = text_of(element, "h4.package-name").unwrap_or_default();
            let href = attr(element, "href").unwrap_or_default();
            let url = absolute_url(&href, &base).unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let summary = text_of(element, "span.package-summary").unwrap_or_default();
            let license = text_of(element, "span.package-license").unwrap_or_default();
            let mut builder =
                ResultBuilder::new(url, title).content(format!("{summary} - {license}"));
            let thumbnail = attr_of(element, "img.package-icon", "src").unwrap_or_default();
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(thumbnail);
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

    const FIXTURE: &str = r#"<html><body>
      <a class="package-header" href="https://f-droid.org/en/packages/org.example.app/">
        <img class="package-icon" src="https://f-droid.org/repo/org.example.app/en-US/icon.png">
        <div>
          <h4 class="package-name">Example App</h4>
          <div>
            <span class="package-summary">An example application.</span>
            <span class="package-license">GPL-3.0-or-later</span>
          </div>
        </div>
      </a>
      <a class="package-header" href="https://f-droid.org/en/packages/org.example.empty/">
        <div><h4 class="package-name"></h4></div>
      </a>
    </body></html>"#;

    fn engine() -> Arc<Fdroid> {
        Fdroid::from_config(&EngineConfig::new(
            "fdroid",
            "fdroid",
            "fd",
            &["it", "apps"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_package_headers() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("example")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Example App");
                assert_eq!(
                    result.url,
                    "https://f-droid.org/en/packages/org.example.app/"
                );
                assert_eq!(result.content, "An example application. - GPL-3.0-or-later");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://f-droid.org/repo/org.example.app/en-US/icon.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let request = engine().build_request(&query("note app"), 2).unwrap();
        assert_eq!(
            request.url,
            "https://search.f-droid.org/?q=note%20app&page=2&lang="
        );
    }

    #[test]
    fn missing_container_is_not_an_error() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query("x")).unwrap().is_empty());
    }
}
