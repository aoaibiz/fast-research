//! Repology engine, ported from searx/engines/repology.py.
//!
//! One result per project; the latest version is the newest one of the
//! repositories, or their most common version.
// engine-id: repology

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
    shortcut: "rep",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://repology.org";

/// Repology package search.
pub struct Repology {
    meta: EngineMeta,
}

impl Repology {
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

impl Engine for Repology {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![("search", query.query_text())];
        let url = format!(
            "{}/api/v1/projects/?{}",
            self.base_url(),
            query_string(&pairs)
        );
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(projects) = json.as_object() else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let mut results = Vec::new();
        for (name, repositories) in projects {
            let Some(repos) = repositories.as_array() else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let url = format!("{base}/project/{name}/versions");
            if !is_http_url(&url) {
                continue;
            }
            let mut version = most_common(repos.iter().map(|repo| json_str(repo, "version")));
            for repo in repos {
                if json_str(repo, "status") == "newest" {
                    version = Some(json_str(repo, "version"));
                    break;
                }
            }
            let content = most_common(repos.iter().map(|repo| json_str(repo, "summary")));
            let package_name = most_common(repos.iter().map(|repo| json_str(repo, "visiblename")));
            let licenses = repos.iter().flat_map(|repo| {
                json_path(repo, "licenses")
                    .and_then(|value| value.as_array())
                    .cloned()
                    .unwrap_or_default()
            });
            let license_name =
                most_common(licenses.filter_map(|value| value.as_str().map(str::to_string)));
            let mut tags: Vec<String> = Vec::new();
            for repo in repos {
                let tag = json_str(repo, "repo");
                if !tag.is_empty() && !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
            let mut builder = ResultBuilder::new(url, name.clone())
                .content(content.unwrap_or_default())
                .template(Template::Packages)
                .metadata("package_name", package_name.unwrap_or_default())
                .metadata("license_name", license_name.unwrap_or_default())
                .metadata("tags", tags.join(", "));
            if let Some(version) = version {
                builder = builder.metadata("version", version);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The most common non-empty value, the first one wins on a tie (upstream
/// compares the counts the same way).
fn most_common(values: impl Iterator<Item = String>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for value in values {
        if value.is_empty() {
            continue;
        }
        if let Some(index) = counts.iter().position(|(key, _)| *key == value) {
            counts[index].1 += 1;
        } else {
            counts.push((value, 1));
        }
    }
    // A stable sort keeps the first seen value on a tie, like upstream.
    let mut ranked = counts;
    ranked.sort_by_key(|item| std::cmp::Reverse(item.1));
    ranked.into_iter().next().map(|(value, _)| value)
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::SearchResult;

    const FIXTURE: &str = r#"{
      "firefox": [
        {"repo": "debian", "version": "120.0", "status": "newest", "summary": "Mozilla Firefox", "visiblename": "firefox", "licenses": ["MPL-2.0"]},
        {"repo": "arch", "version": "121.0", "status": "outdated", "summary": "Mozilla Firefox", "visiblename": "firefox", "licenses": ["MPL-2.0"]}
      ],
      "curl": [
        {"repo": "deb", "version": "8.0", "status": "outdated", "summary": "CLI tool", "visiblename": "curl", "licenses": ["MIT", "curl"]},
        {"repo": "rpm", "version": "8.0", "status": "outdated", "summary": "CLI tool", "visiblename": "curl", "licenses": ["MIT"]}
      ]
    }"#;

    fn engine() -> Arc<Repology> {
        let config = EngineConfig::new("repology", "repology", "rep", &["packages", "it"]);
        Repology::from_config(&config).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn result_of<'a>(items: &'a [EngineItem], title: &str) -> &'a SearchResult {
        items
            .iter()
            .find_map(|item| match item {
                EngineItem::Result(result) if result.title == title => Some(result),
                _ => None,
            })
            .expect("result")
    }

    #[test]
    fn parses_projects() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("firefox")).unwrap();
        assert_eq!(items.len(), 2);
        let firefox = result_of(&items, "firefox");
        assert_eq!(firefox.url, "https://repology.org/project/firefox/versions");
        assert_eq!(firefox.content, "Mozilla Firefox");
        assert_eq!(firefox.template, Template::Packages);
        let metadata = |key: &str| firefox.metadata.get(key).cloned().unwrap_or_default();
        assert_eq!(metadata("version"), "120.0");
        assert_eq!(metadata("package_name"), "firefox");
        assert_eq!(metadata("license_name"), "MPL-2.0");
        assert_eq!(metadata("tags"), "debian, arch");
        let curl = result_of(&items, "curl");
        let metadata = |key: &str| curl.metadata.get(key).cloned().unwrap_or_default();
        assert_eq!(metadata("version"), "8.0");
        assert_eq!(metadata("license_name"), "MIT");
        assert_eq!(metadata("tags"), "deb, rpm");
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query("firefox"), 1).unwrap();
        assert_eq!(
            request.url,
            "https://repology.org/api/v1/projects/?search=firefox"
        );
    }
}
