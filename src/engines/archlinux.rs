//! Archlinux engine, ported from searx/engines/archlinux.py.
//!
//! The Arch Wiki has no usable API, so the Special:Search page is parsed. The
//! language traits of upstream (wiki host, translated title, wiki language)
//! are constants here.

// engine-id: archlinux

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "software wikis"],
    shortcut: "al",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const MAIN_WIKI: &str = "wiki.archlinux.org";
const PAGE_SIZE: u32 = 20;

/// Arch Linux Wiki.
pub struct Archlinux {
    meta: EngineMeta,
}

impl Archlinux {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Archlinux {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let (endpoint, title, language_name, main) = wiki_endpoint(&self.meta, query);
        let mut search = query.query_text();
        if main {
            // Upstream biases the English wiki search with the wiki language.
            search = format!("{search} ({language_name})");
        }
        let size = page_size(&self.meta);
        let pairs = vec![
            ("search", search),
            ("title", title.to_string()),
            ("limit", size.to_string()),
            ("offset", (pageno.saturating_sub(1) * size).to_string()),
            ("profile", "default".to_string()),
        ];
        let mut request = EngineRequest::get(format!("{endpoint}?{}", query_string(&pairs)));
        if main {
            // wiki.archlinux.org is protected by anubis; upstream sends the
            // SearXNG user agent for the main wiki only.
            request.set_header("User-Agent", &crate::network::default_user_agent());
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        // Upstream reuses index.php as the base when resolving result links.
        let (endpoint, ..) = wiki_endpoint(&self.meta, query);
        let mut results = Vec::new();
        for item in select_elements(&html, "ul.mw-search-results > li")? {
            let Some(link) = child(item, "div.mw-search-result-heading a") else {
                continue;
            };
            let (Some(href), title) = (attr(link, "href"), text(link)) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let Some(url) = absolute_url(&href, &endpoint) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let content = text_of(item, "div.searchresult").unwrap_or_default();
            results.push(ResultBuilder::new(url, title).content(content).build());
        }
        Ok(results)
    }
}

/// The index.php endpoint, search title, wiki language and main wiki flag.
fn wiki_endpoint(meta: &EngineMeta, query: &Query) -> (String, &'static str, &'static str, bool) {
    if let Some(base) = &meta.base_url {
        let base = base.trim_end_matches('/');
        return (format!("{base}/index.php"), "Special:Search", "", false);
    }
    let language = locale_language(&query.locale);
    let language = if language == "all" {
        "en".to_string()
    } else {
        language
    };
    let (netloc, title) = wiki_target(&language);
    let path = if netloc == "wiki.archlinuxcn.org" {
        "/wzh/index.php"
    } else {
        "/index.php"
    };
    (
        format!("https://{netloc}{path}"),
        title,
        wiki_language(&language),
        netloc == MAIN_WIKI,
    )
}

/// Upstream traits.custom: the wiki host and the translated search title.
fn wiki_target(language: &str) -> (&'static str, &'static str) {
    match language {
        "de" => ("wiki.archlinux.de", "Spezial:Suche"),
        "ja" => ("wiki.archlinux.jp", "特別:検索"),
        "zh" => ("wiki.archlinuxcn.org", "Special:搜索"),
        _ => (MAIN_WIKI, "Special:Search"),
    }
}

/// Upstream traits.languages: the wiki language shown for each locale.
fn wiki_language(language: &str) -> &'static str {
    const NAMES: &[(&str, &str)] = &[
        ("ar", "العربية"),
        ("bg", "Български"),
        ("bs", "Bosanski"),
        ("ca", "Català"),
        ("cs", "Čeština"),
        ("da", "Dansk"),
        ("de", "Deutsch"),
        ("el", "Ελληνικά"),
        ("en", "English"),
        ("es", "Español"),
        ("fi", "Suomi"),
        ("fr", "Français"),
        ("he", "עברית"),
        ("hr", "Hrvatski"),
        ("hu", "Magyar"),
        ("id", "Bahasa Indonesia"),
        ("it", "Italiano"),
        ("ja", "日本語"),
        ("ko", "한국어"),
        ("lt", "Lietuvių"),
        ("lv", "Latviešu"),
        ("nl", "Nederlands"),
        ("pl", "Polski"),
        ("pt", "Português"),
        ("ru", "Русский"),
        ("sk", "Slovenčina"),
        ("sr", "Српски / srpski"),
        ("sv", "Svenska"),
        ("th", "ไทย"),
        ("tr", "Türkçe"),
        ("uk", "Українська"),
        ("zh", "中文（繁體）"),
    ];
    NAMES
        .iter()
        .find(|(code, _)| *code == language)
        .map(|(_, name)| *name)
        .unwrap_or("English")
}

fn page_size(meta: &EngineMeta) -> u32 {
    meta.opt_i64("page_size")
        .filter(|value| *value > 0)
        .map_or(PAGE_SIZE, |value| value as u32)
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><body><ul class="mw-search-results">
  <li><div class="mw-search-result-heading"><a href="/index.php/Installation_guide">Installation guide</a></div>
    <div class="searchresult">This article covers installing Arch Linux.</div></li>
  <li><div class="mw-search-result-heading"><a href="/index.php/Pacman">pacman</a></div>
    <div class="searchresult">The package manager.</div></li>
</ul></body></html>"##;

    fn engine() -> Arc<Archlinux> {
        let config = EngineConfig::new(
            "arch linux wiki",
            "archlinux",
            "al",
            &["it", "software wikis"],
        );
        Archlinux::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_search_page() {
        let response = EngineResponse::new(200, MAIN_WIKI, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Installation guide");
                assert_eq!(
                    result.url,
                    "https://wiki.archlinux.org/index.php/Installation_guide"
                );
                assert_eq!(result.content, "This article covers installing Arch Linux.");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_request() {
        let request = engine().build_request(&query(), 3).unwrap();
        assert!(
            request
                .url
                .starts_with("https://wiki.archlinux.org/index.php?")
        );
        assert!(request.url.contains("search=rust%20%28English%29"));
        assert!(request.url.contains("title=Special%3ASearch"));
        assert!(request.url.contains("limit=20"));
        assert!(request.url.contains("offset=40"));
        assert!(request.headers.contains_key("user-agent"));
    }

    #[test]
    fn follows_the_locale_wiki() {
        let mut query = query();
        query.locale = "de-DE".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://wiki.archlinux.de/index.php?")
        );
        assert!(request.url.contains("title=Spezial%3ASuche"));
        // The German wiki is not the main wiki: no English suffix, no user agent.
        assert!(request.url.contains("search=rust&"));
        assert!(request.headers.get("user-agent").is_none());
    }

    #[test]
    fn missing_container_yields_no_results() {
        let response = EngineResponse::new(200, MAIN_WIKI, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
