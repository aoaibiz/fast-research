//! OpenClipArt engine, ported from `searx/engines/openclipart.py` (images).
// engine-id: openclipart

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
    shortcut: "oca",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://openclipart.org";

/// OpenClipArt.
pub struct OpenClipart {
    meta: EngineMeta,
}

impl OpenClipart {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for OpenClipart {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("query", query.query_text()),
            ("p", pageno.max(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/search/?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let Ok(entries) = select_elements(&html, "div.gallery > div.artwork") else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let Some(href) = attr_of(entry, "a", "href") else {
                continue;
            };
            let Some(src) = attr_of(entry, "a img", "src") else {
                continue;
            };
            let title = attr_of(entry, "a img", "alt").unwrap_or_default();
            let url = absolute_url(&href, BASE_URL).unwrap_or_default();
            let image = absolute_url(&src, BASE_URL).unwrap_or_default();
            if title.is_empty() || !is_http_url(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .image(image)
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

    const FIXTURE: &str = r#"<html><body><div class="gallery">
      <div class="artwork">
        <a href="/detail/1234/lake"><img src="/image/1234/lake.png" alt="Lake"></a>
      </div>
    </div></body></html>"#;

    fn engine() -> Arc<OpenClipart> {
        OpenClipart::from_config(&EngineConfig::new(
            "openclipart",
            "openclipart",
            "oca",
            &["images"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_image_results() {
        let response = EngineResponse::new(
            200,
            "https://openclipart.org/search/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Lake");
                assert_eq!(result.url, "https://openclipart.org/detail/1234/lake");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://openclipart.org/image/1234/lake.png")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("lake", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with("https://openclipart.org/search/?"));
        assert!(request.url.contains("p=2"));
    }
}
