//! Emojipedia engine, ported from searx/engines/emojipedia.py.
//!
//! Emojipedia documents emoji characters; upstream scrapes the search page.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: emojipedia

const BASE_URL: &str = "https://emojipedia.org";

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &[],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// Emojipedia search.
pub struct Emojipedia {
    meta: EngineMeta,
}

impl Emojipedia {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Emojipedia {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        Ok(EngineRequest::get(format!(
            "{base}/search?{}",
            query_string(&[("q", query.query_text().to_string())])
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut results = Vec::new();
        for anchor in select_elements(&html, "div[class^='EmojisList'] a")? {
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, base) else {
                continue;
            };
            let title = text(anchor);
            if !(url.starts_with("http://") || url.starts_with("https://")) || title.is_empty() {
                continue;
            }
            results.push(ResultBuilder::new(url, title).build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Emojipedia {
        let config = EngineConfig::new("emojipedia", "emojipedia", "", &[]);
        match Arc::try_unwrap(Emojipedia::from_config(&config).expect("engine")) {
            Ok(inner) => inner,
            Err(_) => panic!("unique handle"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let query = Query::parse("rocket", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(request.url, "https://emojipedia.org/search?q=rocket");
    }

    #[test]
    fn parses_the_emoji_list() {
        let html = r##"<div class="EmojisList__Section">
            <a href="/rocket"><div class="EmojisList__emoji">Rocket</div></a>
            <a href="/fire"><div>Fire</div></a>
        </div>"##;
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let query = Query::parse("rocket", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://emojipedia.org/rocket");
                assert!(result.title.contains("Rocket"));
            }
            _ => panic!("expected a result"),
        }
    }
}
