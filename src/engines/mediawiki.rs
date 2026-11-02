//! The generic MediaWiki engine, ported from `searx/engines/mediawiki.py`.
//!
//! Every Wikimedia wiki (wikipedia, wikinews, wiktionary, wikisource, ...) is a
//! settings entry of this engine: `base_url` selects the wiki (it may contain
//! `{language}`), the rest maps to the `list=search` API arguments.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// How many results one MediaWiki API page holds (upstream `page_size`).
const DEFAULT_PAGE_SIZE: u32 = 5;

/// A wiki powered by the MediaWiki action API.
pub struct MediaWiki {
    meta: EngineMeta,
    base_url: String,
    search_type: String,
    srenablerewrites: bool,
    srsort: String,
    srprop: String,
    page_size: u32,
    api_path: String,
}

impl MediaWiki {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let base_url = config
            .base_url
            .clone()
            .or_else(|| config.opt_str("base_url").map(str::to_string))
            .ok_or_else(|| format!("mediawiki engine {:?} needs a base_url", config.name))?;
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            base_url,
            search_type: config
                .opt_str("search_type")
                .unwrap_or("nearmatch")
                .to_string(),
            srenablerewrites: config.opt_bool("srenablerewrites").unwrap_or(true),
            srsort: config.opt_str("srsort").unwrap_or("relevance").to_string(),
            srprop: config
                .opt_str("srprop")
                .unwrap_or("snippet|titlesnippet|sectiontitle|categorysnippet|timestamp")
                .to_string(),
            page_size: config
                .opt_i64("page_size")
                .map(|value| value.clamp(1, 50) as u32)
                .unwrap_or(DEFAULT_PAGE_SIZE),
            api_path: config
                .opt_str("api_path")
                .unwrap_or("w/api.php")
                .to_string(),
        }))
    }

    /// The wiki host, with `{language}` substituted from the query locale.
    fn host(&self, query: &Query) -> String {
        let language = locale_language(&query.locale);
        let language = if language == "all" {
            "en".to_string()
        } else {
            language
        };
        self.base_url
            .replace("{language}", &language)
            .trim_end_matches('/')
            .to_string()
    }
}

impl Engine for MediaWiki {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("action", "query".to_string()),
            ("list", "search".to_string()),
            ("format", "json".to_string()),
            ("srsearch", query.query_text()),
            (
                "sroffset",
                ((pageno.max(1) - 1) * self.page_size).to_string(),
            ),
            ("srlimit", self.page_size.to_string()),
            ("srwhat", self.search_type.clone()),
            ("srprop", self.srprop.clone()),
            ("srsort", self.srsort.clone()),
        ];
        if self.srenablerewrites {
            pairs.push(("srenablerewrites", "1".to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{}/{}?{}",
            self.host(query),
            self.api_path.trim_start_matches('/'),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "query.search").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let host = self.host(query);
        let mut results = Vec::new();
        for item in items {
            let snippet = json_str(item, "snippet");
            // Upstream skips redirect stubs, they carry no useful content.
            if snippet.starts_with("#REDIRECT") {
                continue;
            }
            let mut title = json_str(item, "title");
            if title.is_empty() {
                continue;
            }
            let mut url = format!("{}/wiki/{}", host, encode(&title.replace(' ', "_")));
            if let Some(section) = json_path(item, "sectiontitle").and_then(|value| value.as_str())
            {
                if !section.is_empty() {
                    url.push('#');
                    url.push_str(&encode(&section.replace(' ', "_")));
                    title = format!("{title} / {section}");
                }
            }
            let mut builder = ResultBuilder::new(url, title).content(strip_tags(&snippet));
            if let Some(timestamp) = json_path(item, "timestamp").and_then(|value| value.as_str()) {
                builder = builder.published_date(timestamp);
            }
            let category = strip_tags(&json_str(item, "categorysnippet"));
            if !category.is_empty() {
                builder = builder.metadata("category", category);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    const FIXTURE: &str = r##"{"batchcomplete":"","query":{"search":[
      {"ns":0,"title":"Rust (programming language)","snippet":"<span class=\"searchmatch\">Rust</span> is a language.","timestamp":"2026-10-01T00:00:00Z"},
      {"ns":0,"title":"Rust","snippet":"#REDIRECT [[Rust (programming language)]]"},
      {"ns":0,"title":"Rust (disambiguation)","snippet":"Topics.","sectiontitle":"Computing"}
    ]}}"##;

    fn engine() -> Arc<MediaWiki> {
        let mut config = EngineConfig::new("wikipedia", "mediawiki", "wp", &["general"]);
        config.base_url = Some("https://{language}.wikipedia.org".into());
        MediaWiki::from_config(&config).unwrap()
    }

    #[test]
    fn builds_the_action_api_url() {
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let url = engine().build_request(&query, 2).unwrap().url;
        assert!(
            url.starts_with("https://en.wikipedia.org/w/api.php?"),
            "{url}"
        );
        assert!(url.contains("srsearch=rust"), "{url}");
        assert!(url.contains("sroffset=5"), "{url}");
        assert!(url.contains("srlimit=5"), "{url}");
        assert!(url.contains("srwhat=nearmatch"), "{url}");
    }

    #[test]
    fn parses_search_results_and_skips_redirects() {
        let response = EngineResponse::new(
            200,
            "https://en.wikipedia.org/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust (programming language)");
                assert_eq!(
                    result.url,
                    "https://en.wikipedia.org/wiki/Rust_%28programming_language%29"
                );
                assert_eq!(result.content, "Rust is a language.");
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2026-10-01T00:00:00Z")
                );
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Result(result) => {
                assert!(result.url.ends_with("#Computing"));
                assert_eq!(result.title, "Rust (disambiguation) / Computing");
            }
            _ => panic!("expected a result"),
        }
    }
}
