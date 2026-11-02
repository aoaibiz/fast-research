//! Gitea engine, ported from searx/engines/gitea.py.
// engine-id: gitea

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
    shortcut: "gitea",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// Default instance, used when the settings do not name one.
const BASE_URL: &str = "https://gitea.com";

/// Repository search on a Gitea or Forgejo instance.
pub struct Gitea {
    meta: EngineMeta,
}

impl Gitea {
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

    fn sort(&self) -> String {
        self.meta.opt_str("sort").unwrap_or("updated").to_string()
    }

    fn order(&self) -> String {
        self.meta.opt_str("order").unwrap_or("desc").to_string()
    }

    fn page_size(&self) -> i64 {
        self.meta.opt_i64("page_size").unwrap_or(10)
    }
}

impl Engine for Gitea {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.base_url();
        let pairs = vec![
            ("q", query.query_text()),
            ("limit", self.page_size().to_string()),
            ("sort", self.sort()),
            ("order", self.order()),
            ("page", pageno.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{base}/api/v1/repos/search?{}",
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
        let Some(items) = json_path(&json, "data").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "html_url");
            let title = json_str(item, "full_name");
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let mut content = Vec::new();
            for key in ["language", "description"] {
                let value = json_str(item, key);
                if !value.is_empty() {
                    content.push(value);
                }
            }
            let published = {
                let updated = json_str(item, "updated_at");
                if updated.is_empty() {
                    json_str(item, "created_at")
                } else {
                    updated
                }
            };
            let tags = json_path(item, "topics")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let avatar = {
                let avatar = json_str(item, "avatar_url");
                if avatar.is_empty() {
                    json_str(item, "owner.avatar_url")
                } else {
                    avatar
                }
            };
            let mut builder = ResultBuilder::new(url, title)
                .content(content.join(" / "))
                .template(Template::Packages)
                .published_date(published)
                .metadata("package_name", json_str(item, "name"))
                .metadata("maintainer", json_str(item, "owner.username"))
                .metadata("tags", tags)
                .metadata("popularity", json_str(item, "stars_count"))
                .metadata("homepage", json_str(item, "website"))
                .metadata("source_code_url", json_str(item, "clone_url"));
            if !avatar.is_empty() {
                builder = builder.thumbnail(avatar);
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

    const FIXTURE: &str = r##"{"ok":true,"data":[
      {"full_name":"acme/demo","html_url":"https://gitea.example.com/acme/demo","name":"demo",
       "language":"Rust","description":"A demo repo",
       "avatar_url":"https://gitea.example.com/avatars/demo.png",
       "owner":{"username":"acme","avatar_url":"https://gitea.example.com/avatars/acme.png"},
       "updated_at":"2024-05-06T07:08:09Z","created_at":"2023-01-01T00:00:00Z",
       "topics":["rust","cli"],"stars_count":42,"website":"https://demo.example.com",
       "clone_url":"https://gitea.example.com/acme/demo.git"},
      {"full_name":"acme/old","html_url":"https://gitea.example.com/acme/old","name":"old",
       "owner":{"username":"acme","avatar_url":"https://gitea.example.com/avatars/acme.png"},
       "updated_at":"","created_at":"2022-02-02T00:00:00Z"},
      {"name":"no_url"}
    ]}"##;

    fn engine() -> Arc<Gitea> {
        let mut config = EngineConfig::new("gitea", "gitea", "gitea", &["it", "repos"]);
        config.base_url = Some("https://gitea.example.com".to_string());
        Gitea::from_config(&config).unwrap()
    }

    #[test]
    fn parses_repositories() {
        let response = EngineResponse::new(
            200,
            "https://gitea.example.com/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(first) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(first.title, "acme/demo");
        assert_eq!(first.url, "https://gitea.example.com/acme/demo");
        assert_eq!(first.content, "Rust / A demo repo");
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://gitea.example.com/avatars/demo.png")
        );
        assert_eq!(
            first.published_date.as_deref(),
            Some("2024-05-06T07:08:09Z")
        );
        assert_eq!(first.template, Template::Packages);
        let metadata = &first.metadata;
        assert_eq!(
            metadata.get("package_name").map(String::as_str),
            Some("demo")
        );
        assert_eq!(metadata.get("maintainer").map(String::as_str), Some("acme"));
        assert_eq!(metadata.get("tags").map(String::as_str), Some("rust, cli"));
        assert_eq!(metadata.get("popularity").map(String::as_str), Some("42"));
        assert_eq!(
            metadata.get("homepage").map(String::as_str),
            Some("https://demo.example.com")
        );
        assert_eq!(
            metadata.get("source_code_url").map(String::as_str),
            Some("https://gitea.example.com/acme/demo.git")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(
            second.thumbnail.as_deref(),
            Some("https://gitea.example.com/avatars/acme.png")
        );
        assert_eq!(
            second.published_date.as_deref(),
            Some("2022-02-02T00:00:00Z")
        );
    }

    #[test]
    fn builds_the_repository_search_request() {
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://gitea.example.com/api/v1/repos/search?q=demo&limit=10&sort=updated&order=desc&page=2"
        );
        assert_eq!(
            request.headers.get("accept").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn the_public_instance_is_the_default() {
        let config = EngineConfig::new("gitea", "gitea", "gitea", &["it", "repos"]);
        let engine = Gitea::from_config(&config).unwrap();
        let query = Query::parse("demo", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://gitea.com/api/v1/repos/search?")
        );
    }
}
