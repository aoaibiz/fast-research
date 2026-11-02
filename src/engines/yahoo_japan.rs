//! Yahoo! JAPAN web search.
//!
//! Japan's largest portal search. Japanese queries are under-served by the
//! English-centric engines, and this one indexes Japanese official pages well.
// engine-id: yahoo_japan

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "yjp",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE: &str = "https://search.yahoo.co.jp/search";

/// Yahoo! JAPAN web search.
pub struct YahooJapan {
    meta: EngineMeta,
}

impl YahooJapan {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

/// True for a result we want, false for Yahoo's own navigation links.
fn is_organic(url: &str) -> bool {
    if !url.starts_with("http") {
        return false;
    }
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_lowercase();
    !(host.ends_with("yahoo.co.jp") || host.ends_with("yimg.jp") || host.ends_with("lycorp.co.jp"))
}

impl Engine for YahooJapan {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = (pageno.saturating_sub(1)) * 10 + 1;
        let pairs: Vec<(&str, String)> = vec![
            ("p", query.query_text()),
            ("b", offset.to_string()),
            ("ei", "UTF-8".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{BASE}?{}", query_string(&pairs)))
                .header("Accept", "text/html,application/xhtml+xml"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let mut results = Vec::new();
        for card in select_elements(&html, "div.sw-Card")? {
            let Some(url) = attr_of(card, "a.sw-Card__titleInner", "href") else {
                continue;
            };
            if !is_organic(&url) {
                continue;
            }
            let title = text_of(card, "h3").unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let content = text_of(card, "p").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body>
      <div class="sw-Card Algo">
        <a class="sw-Card__titleInner" href="https://note.nkmk.me/python-dict-get/">
          <h3 class="sw-Card__titleMain">Pythonの辞書のgetメソッド</h3>
        </a>
        <p class="sw-Card__summary">辞書からキーの値を取得します。</p>
      </div>
      <div class="sw-Card Algo">
        <a class="sw-Card__titleInner" href="https://search.yahoo.co.jp/search/preferences">
          <h3>設定</h3>
        </a>
      </div>
      <div class="sw-Card Algo">
        <a class="sw-Card__titleInner" href="https://qiita.com/shinsaka/items/80d">
          <h3>Python:辞書型のget()メソッド</h3>
        </a>
      </div>
    </body></html>"##;

    #[test]
    fn parses_results_and_skips_yahoo_links() {
        let response = EngineResponse::new(200, BASE, FIXTURE.as_bytes().to_vec());
        let registry = crate::engine::EngineRegistry::default();
        let query = Query::parse("python dict get", &registry, &[]);
        let config = EngineConfig::new("yahoo_japan", "yahoo_japan", "yjp", &["general"]);
        let engine = YahooJapan::from_config(&config).expect("engine");
        let items = engine.parse(&response, &query).expect("parse");
        assert_eq!(items.len(), 2, "the Yahoo preferences link is skipped");
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://note.nkmk.me/python-dict-get/");
                assert!(result.title.contains("get"));
                assert!(result.content.contains("辞書"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn paging_moves_the_offset() {
        let config = EngineConfig::new("yahoo_japan", "yahoo_japan", "yjp", &["general"]);
        let engine = YahooJapan::from_config(&config).expect("engine");
        let registry = crate::engine::EngineRegistry::default();
        let query = Query::parse("test", &registry, &[]);
        let first = engine.build_request(&query, 1).expect("request");
        let second = engine.build_request(&query, 2).expect("request");
        assert!(first.url.contains("b=1"), "{}", first.url);
        assert!(second.url.contains("b=11"), "{}", second.url);
    }
}
