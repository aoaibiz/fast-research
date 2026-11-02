//! hex.pm engine, ported from searx/engines/hex.py.
// engine-id: hex

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
    shortcut: "hex",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://hex.pm/api/packages/";
const PAGE_SIZE: i64 = 10;

/// Erlang and Elixir packages on hex.pm.
pub struct Hex {
    meta: EngineMeta,
}

impl Hex {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// Valid values: name, inserted_at, updated_at, total_downloads,
    /// recent_downloads.
    fn sort_criteria(&self) -> String {
        self.meta
            .opt_str("sort_criteria")
            .unwrap_or("recent_downloads")
            .to_string()
    }

    fn page_size(&self) -> i64 {
        self.meta.opt_i64("page_size").unwrap_or(PAGE_SIZE)
    }
}

impl Engine for Hex {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("page", pageno.to_string()),
            ("per_page", self.page_size().to_string()),
            ("sort", self.sort_criteria()),
            ("search", query.query_text()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(packages) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for package in packages {
            let name = json_str(package, "name");
            let url = json_str(package, "html_url");
            if !is_http_url(&url) || name.is_empty() {
                continue;
            }
            let links = json_path(package, "meta.links")
                .and_then(|value| value.as_object())
                .map(|links| {
                    links
                        .iter()
                        .map(|(key, value)| {
                            format!(
                                "{}: {}",
                                linked_term(key),
                                value.as_str().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, name.clone())
                    .template(Template::Packages)
                    .content(json_str(package, "meta.description"))
                    .published_date(json_str(package, "updated_at"))
                    .metadata("package_name", name)
                    .metadata("version", json_str(package, "meta.latest_version"))
                    .metadata("maintainer", string_array(package, "meta.maintainers"))
                    .metadata("license_name", string_array(package, "meta.licenses"))
                    .metadata("homepage", json_str(package, "docs_html_url"))
                    .metadata("links", links)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// The human readable label of a hex.pm link key, upstream linked_terms.
fn linked_term(key: &str) -> String {
    let label = match key.to_lowercase().as_str() {
        "author" => "Author",
        "bitbucket" => "Bitbucket",
        "bug tracker" | "issues" => "Issue tracker",
        "changelog" => "Changelog",
        "doc" | "docs" | "documentation" => "Documentation",
        "github repository" | "github" => "GitHub",
        "gitlab" => "GitLab",
        "project source code" | "repository" | "scm" | "sources" => "Source code",
        "sourcehut" => "SourceHut",
        "sponsor" | "sponsors" => "Sponsors",
        "website" => "Homepage",
        _ => return key.to_string(),
    };
    label.to_string()
}

fn string_array(value: &serde_json::Value, path: &str) -> String {
    json_path(value, path)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"[
      {"name":"plug","html_url":"https://hex.pm/packages/plug",
       "docs_html_url":"https://hexdocs.pm/plug/","updated_at":"2024-01-02T03:04:05Z",
       "meta":{"description":"A web framework","latest_version":"1.15.0",
               "maintainers":["alice","bob"],"licenses":["Apache-2.0"],
               "links":{"GitHub":"https://github.com/elixir-plug/plug",
                        "Website":"https://example.com"}}},
      {"name":"broken"}
    ]"##;

    fn engine() -> Arc<Hex> {
        Hex::from_config(&EngineConfig::new("hex", "hex", "hex", &["it", "packages"])).unwrap()
    }

    #[test]
    fn parses_packages() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("plug", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "plug");
        assert_eq!(result.url, "https://hex.pm/packages/plug");
        assert_eq!(result.content, "A web framework");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-01-02T03:04:05Z")
        );
        assert_eq!(result.template, Template::Packages);
        let metadata = &result.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("plug")
        );
        assert_eq!(metadata.get("version").map(String::as_str), Some("1.15.0"));
        assert_eq!(
            metadata.get("maintainer").map(String::as_str),
            Some("alice, bob")
        );
        assert_eq!(
            metadata.get("license_name").map(String::as_str),
            Some("Apache-2.0")
        );
        assert_eq!(
            metadata.get("homepage").map(String::as_str),
            Some("https://hexdocs.pm/plug/")
        );
        assert_eq!(
            metadata.get("links").map(String::as_str),
            Some("GitHub: https://github.com/elixir-plug/plug | Homepage: https://example.com")
        );
    }

    #[test]
    fn builds_the_package_search_request() {
        let query = Query::parse("plug", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://hex.pm/api/packages/?page=2&per_page=10&sort=recent_downloads&search=plug"
        );
        assert_eq!(
            request.headers.get("accept").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, SEARCH_URL, b"{}".to_vec());
        let query = Query::parse("plug", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
