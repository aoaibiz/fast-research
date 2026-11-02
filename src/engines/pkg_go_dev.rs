//! pkg.go.dev engine, ported from searx/engines/pkg_go_dev.py.
// engine-id: pkg_go_dev

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["packages", "it"],
    shortcut: "pgo",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://pkg.go.dev";
const MAX_RESULT_COUNT: i64 = 50;

/// Go package search on pkg.go.dev.
pub struct PkgGoDev {
    meta: EngineMeta,
}

impl PkgGoDev {
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

impl Engine for PkgGoDev {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("m", "package".to_string()),
            ("limit", MAX_RESULT_COUNT.to_string()),
        ];
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
        let html = parse_html(&response.text());
        let base = self.base_url();
        let containers = select_elements(&html, "div.SearchResults")?;
        let Some(container) = containers.first() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in children(*container, "div.SearchSnippet") {
            let Some(link) = child(item, "div.SearchSnippet-headerContainer h2 a") else {
                continue;
            };
            let href = attr(link, "href").unwrap_or_default();
            if href.is_empty() {
                continue;
            }
            // Upstream reads only the direct text of the anchor (the package
            // path), the span holds the name without the module path.
            let title = {
                let parts: Vec<String> = link
                    .children()
                    .filter_map(|node| node.value().as_text().map(|text| text.to_string()))
                    .collect();
                clean(&parts.join(" "))
            };
            let url = if href.starts_with("http") {
                href
            } else {
                format!("{base}{href}")
            };
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let package_name: String = text_of(link, "span")
                .unwrap_or_default()
                .chars()
                .filter(|character| *character != '(' && *character != ')')
                .collect();
            let license =
                "div[class*='SearchSnippet-infoLabel'] span[data-test-id='snippet-license'] a";
            let license_url = attr_of(item, license, "href")
                .map(|href| {
                    if href.starts_with("http") {
                        href
                    } else {
                        format!("{base}{href}")
                    }
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, title)
                    .template(Template::Packages)
                    .content(text_of(item, "p.SearchSnippet-synopsis").unwrap_or_default())
                    .published_date(
                        text_of(item, "div[class*='SearchSnippet-infoLabel'] span[data-test-id='snippet-published'] strong")
                            .unwrap_or_default(),
                    )
                    .metadata("package_name", package_name)
                    .metadata("version", text_of(item, "div[class*='SearchSnippet-infoLabel'] > span > strong").unwrap_or_default())
                    .metadata("popularity", text_of(item, "div[class*='SearchSnippet-infoLabel'] > a > strong").unwrap_or_default())
                    .metadata("license_name", text_of(item, license).unwrap_or_default())
                    .metadata("license_url", license_url)
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

    const FIXTURE: &str = r##"<html><body><main>
      <div class="SearchResults"><div>
        <div class="SearchSnippet">
          <div class="SearchSnippet-headerContainer"><h2><a href="/github.com/acme/demo">acme/demo<span>(demo)</span></a></h2></div>
          <p class="SearchSnippet-synopsis">A demo module for Go</p>
          <div class="SearchSnippet-infoLabel">
            <span><strong>v1.2.3</strong></span>
            <span data-test-id="snippet-published"><strong>Jan 2, 2024</strong></span>
            <a href="/github.com/acme/demo?tab=importedby"><strong>1,234</strong></a>
            <span data-test-id="snippet-license"><a href="https://opensource.org/licenses/MIT">MIT</a></span>
          </div>
        </div>
        <div class="SearchSnippet"><p>broken</p></div>
      </div></div>
    </main></body></html>"##;

    fn engine() -> Arc<PkgGoDev> {
        PkgGoDev::from_config(&EngineConfig::new(
            "pkg go dev",
            "pkg_go_dev",
            "pgo",
            &["packages", "it"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_search_snippets() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "acme/demo");
        assert_eq!(result.url, "https://pkg.go.dev/github.com/acme/demo");
        assert_eq!(result.content, "A demo module for Go");
        assert_eq!(result.published_date.as_deref(), Some("Jan 2, 2024"));
        assert_eq!(result.template, Template::Packages);
        let metadata = &result.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("demo")
        );
        assert_eq!(metadata.get("version").map(String::as_str), Some("v1.2.3"));
        assert_eq!(
            metadata.get("popularity").map(String::as_str),
            Some("1,234")
        );
        assert_eq!(
            metadata.get("license_name").map(String::as_str),
            Some("MIT")
        );
        assert_eq!(
            metadata.get("license_url").map(String::as_str),
            Some("https://opensource.org/licenses/MIT")
        );
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://pkg.go.dev/search?q=demo&m=package&limit=50"
        );
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
