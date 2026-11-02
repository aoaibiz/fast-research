//! lib.rs engine, ported from searx/engines/lib_rs.py.
// engine-id: lib_rs

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "packages"],
    shortcut: "lrs",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://lib.rs";

/// lib.rs crate search.
pub struct LibRs {
    meta: EngineMeta,
}

impl LibRs {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for LibRs {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("q", query.query_text())];
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
        // Upstream selects main/div/ol/li/a and needs the header block of the crate.
        for item in select_elements(&html, "main ol li > a")? {
            if child(item, "div.h").is_none() {
                continue;
            }
            let Some(href) = attr(item, "href") else {
                continue;
            };
            let url = if href.starts_with("http") {
                href
            } else {
                format!("{base}{href}")
            };
            let title = text_of(item, "div.h h4").unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let tags: Vec<String> = children(item, "div.meta span[class~='k']")
                .iter()
                .map(|element| text(*element))
                .filter(|tag| !tag.is_empty())
                .collect();
            results.push(
                ResultBuilder::new(url, title.clone())
                    .template(Template::Packages)
                    .content(text_of(item, "div.h p").unwrap_or_default())
                    .metadata("package_name", title)
                    .metadata(
                        "version",
                        text_of(item, "div.meta span[class~='version']").unwrap_or_default(),
                    )
                    .metadata(
                        "popularity",
                        text_of(item, "div.meta span.downloads").unwrap_or_default(),
                    )
                    .metadata("tags", tags.join(", "))
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

    const FIXTURE: &str = r##"<html><body><main><div><ol>
      <li><a href="/crate/serde"><div class="h"><h4>serde</h4><p>Serialization framework</p></div><div class="meta"><span class="version">1.0.200</span><span class="downloads">1,234</span><span class="k">no_std</span><span class="k">serde</span></div></a></li>
      <li><a href="/crate/broken"><p>No header</p></a></li>
    </ol></div></main></body></html>"##;

    fn engine() -> Arc<LibRs> {
        LibRs::from_config(&EngineConfig::new(
            "lib.rs",
            "lib_rs",
            "lrs",
            &["it", "packages"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_crates() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("serde")).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "serde");
        assert_eq!(result.url, "https://lib.rs/crate/serde");
        assert_eq!(result.content, "Serialization framework");
        assert_eq!(result.template, Template::Packages);
        let metadata = &result.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("serde")
        );
        assert_eq!(metadata.get("version").map(String::as_str), Some("1.0.200"));
        assert_eq!(
            metadata.get("popularity").map(String::as_str),
            Some("1,234")
        );
        assert_eq!(
            metadata.get("tags").map(String::as_str),
            Some("no_std, serde")
        );
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("serde json"), 1).unwrap();
        assert_eq!(request.url, "https://lib.rs/search?q=serde%20json");
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        assert!(
            engine()
                .parse(&response, &query("serde"))
                .unwrap()
                .is_empty()
        );
    }
}
