//! GitLab engine, ported from searx/engines/gitlab.py.
// engine-id: gitlab

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
    shortcut: "gl",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// Default host; the API path is configurable like upstream's api_path.
const BASE_URL: &str = "https://gitlab.com";
const API_PATH: &str = "api/v4/projects";

/// Project search of a GitLab host.
pub struct GitLab {
    meta: EngineMeta,
}

impl GitLab {
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

    fn api_path(&self) -> String {
        self.meta
            .opt_str("api_path")
            .unwrap_or(API_PATH)
            .trim_matches('/')
            .to_string()
    }
}

impl Engine for GitLab {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![("search", query.query_text()), ("page", pageno.to_string())];
        Ok(EngineRequest::get(format!(
            "{}/{}?{}",
            self.base_url(),
            self.api_path(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json.as_array() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "web_url");
            let name = json_str(item, "name");
            if !is_http_url(&url) || name.is_empty() {
                continue;
            }
            let mut published = json_str(item, "last_activity_at");
            if published.is_empty() {
                published = json_str(item, "created_at");
            }
            let mut builder = ResultBuilder::new(url, name.clone())
                .content(json_str(item, "description"))
                .template(Template::Packages)
                .published_date(published)
                .metadata("package_name", name)
                .metadata("maintainer", json_str(item, "namespace.name"))
                .metadata("popularity", json_str(item, "star_count"))
                .metadata("tags", string_array(item, "tag_list"))
                .metadata("homepage", json_str(item, "readme_url"))
                .metadata("source_code_url", json_str(item, "http_url_to_repo"));
            let avatar = json_str(item, "avatar_url");
            if !avatar.is_empty() {
                builder = builder.thumbnail(avatar);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

/// The string elements of a JSON array, joined the way upstream joins tags.
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
      {"name":"demo","web_url":"https://gitlab.example.com/group/demo",
       "description":"A demo project","avatar_url":"https://gitlab.example.com/avatar.png",
       "namespace":{"name":"Group"},"last_activity_at":"2024-02-03T04:05:06Z",
       "created_at":"2023-01-01T00:00:00Z","tag_list":["rust","cli"],"star_count":42,
       "readme_url":"https://gitlab.example.com/group/demo/-/blob/main/README.md",
       "http_url_to_repo":"https://gitlab.example.com/group/demo.git"},
      {"name":"broken","web_url":""}
    ]"##;

    fn engine() -> Arc<GitLab> {
        let mut config = EngineConfig::new("gitlab", "gitlab", "gl", &["it", "repos"]);
        config.base_url = Some("https://gitlab.example.com".to_string());
        GitLab::from_config(&config).unwrap()
    }

    #[test]
    fn parses_projects() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "demo");
        assert_eq!(result.url, "https://gitlab.example.com/group/demo");
        assert_eq!(result.content, "A demo project");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-02-03T04:05:06Z")
        );
        assert_eq!(result.template, Template::Packages);
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://gitlab.example.com/avatar.png")
        );
        let metadata = &result.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("demo")
        );
        assert_eq!(
            metadata.get("maintainer").map(String::as_str),
            Some("Group")
        );
        assert_eq!(metadata.get("popularity").map(String::as_str), Some("42"));
        assert_eq!(metadata.get("tags").map(String::as_str), Some("rust, cli"));
        assert_eq!(
            metadata.get("source_code_url").map(String::as_str),
            Some("https://gitlab.example.com/group/demo.git")
        );
    }

    #[test]
    fn builds_the_projects_api_request() {
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://gitlab.example.com/api/v4/projects?search=demo&page=2"
        );
        assert_eq!(
            request.headers.get("accept").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        assert!(engine().parse(&response, &query).unwrap().is_empty());
    }
}
