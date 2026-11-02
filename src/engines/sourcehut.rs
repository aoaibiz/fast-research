//! SourceHut engine, ported from searx/engines/sourcehut.py.
// engine-id: sourcehut

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "repos"],
    shortcut: "srht",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://sr.ht/projects";
/// SourceHut blocks the standard user agents with its go-away filter.
const USER_AGENT: &str = "rustxng/0.1 (+https://github.com/searxng/searxng)";

/// Project search on SourceHut.
pub struct SourceHut {
    meta: EngineMeta,
}

impl SourceHut {
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

    /// recently-updated (default) or longest-active.
    fn sort_order(&self) -> String {
        self.meta
            .opt_str("sourcehut_sort_order")
            .unwrap_or("recently-updated")
            .to_string()
    }
}

impl Engine for SourceHut {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("search", query.query_text()),
            ("page", pageno.to_string()),
            ("sort", self.sort_order()),
        ];
        Ok(
            EngineRequest::get(format!("{}?{}", self.base_url(), query_string(&pairs)))
                .header("User-Agent", USER_AGENT),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.base_url();
        // Upstream only reads the first event list of the page.
        let lists = select_elements(&html, "div.event-list")?;
        let Some(list) = lists.first() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in children(*list, "div[class*='event']") {
            let Some(heading) = child(item, "h4") else {
                continue;
            };
            // The first link is the owner, the second one the project.
            let links = children(heading, "a");
            let Some(project) = links.get(1) else {
                continue;
            };
            let href = attr(*project, "href").unwrap_or_default();
            let url = absolute_url(&href, &base).unwrap_or_default();
            let title = text(heading);
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let maintainer = links.first().map(|link| text(*link)).unwrap_or_default();
            let tags = children(item, "div[class*='tags'] a")
                .iter()
                .map(|tag| text(*tag))
                .map(|tag| tag.trim_start_matches('#').to_string())
                .filter(|tag| !tag.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            results.push(
                ResultBuilder::new(url, title)
                    .template(Template::Packages)
                    .content(text_of(item, "p").unwrap_or_default())
                    .metadata("package_name", text(*project))
                    .metadata("maintainer", maintainer.trim_start_matches('~'))
                    .metadata("tags", tags)
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

    const FIXTURE: &str = r##"<html><body>
      <div class="event-list">
        <div class="event">
          <h4><a href="/~alice">~alice</a> / <a href="/~alice/demo">demo</a></h4>
          <p>A tiny demo project</p>
          <div class="tags"><a href="/t/rust">#rust</a><a href="/t/cli">#cli</a></div>
        </div>
        <div class="event"><p>no heading</p></div>
      </div>
      <div class="event-list">
        <div class="event"><h4><a href="/~bob">~bob</a> / <a href="/~bob/other">other</a></h4></div>
      </div>
    </body></html>"##;

    fn engine() -> Arc<SourceHut> {
        SourceHut::from_config(&EngineConfig::new(
            "sourcehut",
            "sourcehut",
            "srht",
            &["it", "repos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_the_first_event_list() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "~alice / demo");
        assert_eq!(result.url, "https://sr.ht/~alice/demo");
        assert_eq!(result.content, "A tiny demo project");
        assert_eq!(result.template, Template::Packages);
        let metadata = &result.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("demo")
        );
        assert_eq!(
            metadata.get("maintainer").map(String::as_str),
            Some("alice")
        );
        assert_eq!(metadata.get("tags").map(String::as_str), Some("rust, cli"));
    }

    #[test]
    fn builds_the_project_search_request() {
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://sr.ht/projects?search=demo&page=2&sort=recently-updated"
        );
        assert_eq!(
            request.headers.get("user-agent").unwrap().to_str().unwrap(),
            USER_AGENT
        );
    }

    #[test]
    fn a_missing_event_list_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html><body></body></html>".to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
