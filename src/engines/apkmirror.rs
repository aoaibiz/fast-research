//! APKMirror engine, ported from `searx/engines/apkmirror.py`.
// engine-id: apkmirror

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "apps"],
    shortcut: "apk",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.apkmirror.com";

/// APKMirror.
pub struct ApkMirror {
    meta: EngineMeta,
}

impl ApkMirror {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for ApkMirror {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("s", query.query_text())];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/?post_type=app_release&searchtype=apk&page={}&{}",
            pageno.max(1),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let Ok(entries) = select_elements(&html, "div#content div.listWidget div div.appRow")
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let Some(href) = attr_of(entry, "h5 a", "href") else {
                continue;
            };
            let title = text_of(entry, "h5 a").unwrap_or_default();
            let url = absolute_url(&format!("{href}#downloads"), BASE_URL).unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder = ResultBuilder::new(url, title);
            if let Some(src) = attr_of(entry, "img", "src") {
                if let Some(thumbnail) = absolute_url(&src, BASE_URL) {
                    builder = builder.thumbnail(thumbnail);
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

    const FIXTURE: &str = r#"<html><body><div id="content">
      <div class="listWidget">
        <div><div class="appRow">
          <h5><a href="/apk/jane-doe/rust-app/">Rust App 1.2</a></h5>
          <img src="/wp-content/uploads/rust.png">
        </div></div>
      </div>
    </div></body></html>"#;

    fn engine() -> Arc<ApkMirror> {
        ApkMirror::from_config(&EngineConfig::new(
            "apkmirror",
            "apkmirror",
            "apk",
            &["it", "apps"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_app_results() {
        let response = EngineResponse::new(
            200,
            "https://www.apkmirror.com/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust App 1.2");
                assert_eq!(
                    result.url,
                    "https://www.apkmirror.com/apk/jane-doe/rust-app/#downloads"
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://www.apkmirror.com/wp-content/uploads/rust.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("rust app", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.apkmirror.com/?post_type=app_release")
        );
        assert!(request.url.contains("searchtype=apk"));
        assert!(request.url.contains("page=2"));
    }
}
