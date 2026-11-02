//! GitHub code search engine, ported from searx/engines/github_code.py.
// engine-id: github_code

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["code"],
    shortcut: "ghc",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const ACCEPT_HEADER: &str = "application/vnd.github.text-match+json";
/// Lines are split on this, upstream splits on the newline character.
const NEWLINE: &str = "\n";

/// GitHub code search through the REST search API.
pub struct GitHubCode {
    meta: EngineMeta,
}

impl GitHubCode {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The ghc_auth type: none, personal_access_token or bearer.
    fn auth_type(&self) -> String {
        self.meta
            .opt_str("ghc_auth_type")
            .unwrap_or("none")
            .to_lowercase()
    }

    /// Split the returned text matches into plain code lines. Upstream also
    /// tracks highlighted lines; a plain result has no place for them.
    fn extract_code(&self, item: &serde_json::Value) -> Vec<String> {
        let strip_whitespace = self.meta.opt_bool("ghc_strip_whitespace").unwrap_or(false);
        let strip_new_lines = self.meta.opt_bool("ghc_strip_new_lines").unwrap_or(true);
        let separator = self
            .meta
            .opt_bool("ghc_insert_block_separator")
            .unwrap_or(false);
        let mut lines = Vec::new();
        let Some(matches) = item.get("text_matches").and_then(|value| value.as_array()) else {
            return lines;
        };
        let mut first = true;
        for entry in matches {
            if json_str(entry, "object_type") != "FileContent"
                || json_str(entry, "property") != "content"
            {
                continue;
            }
            let mut fragment = json_str(entry, "fragment");
            if strip_whitespace {
                fragment = fragment.trim().to_string();
            }
            if strip_new_lines {
                fragment = strip_newlines(&fragment);
            }
            if !first && separator {
                lines.push("...".to_string());
            }
            first = false;
            for line in fragment.split(NEWLINE) {
                lines.push(line.to_string());
            }
        }
        lines
    }
}

impl Engine for GitHubCode {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        let base = base.trim_end_matches('/');
        let url = format!(
            "{base}/search/code?sort=indexed&{}&{}",
            query_string(&[("q", query.query_text())]),
            query_string(&[("page", pageno.to_string())])
        );
        let mut request = EngineRequest::get(url)
            .header("Accept", ACCEPT_HEADER)
            .header("X-GitHub-Api-Version", API_VERSION);
        // The token belongs to the auth type and is stored as api_key.
        let token = self.meta.api_key.clone().unwrap_or_default();
        request = match self.auth_type().as_str() {
            "personal_access_token" => request.header("Authorization", &format!("token {token}")),
            "bearer" => request.header("Authorization", &format!("Bearer {token}")),
            // Upstream sends a dummy header, an unauthenticated call fails.
            _ => request.header("Authorization", "placeholder"),
        };
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        // A malformed search term makes GitHub answer 422; nothing to collect.
        if response.status == 422 {
            return Ok(Vec::new());
        }
        let json = response.json()?;
        let Some(items) = json_path(&json, "items").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "html_url");
            let name = json_str(item, "name");
            if !is_http_url(&url) || name.is_empty() {
                continue;
            }
            let repository = item.get("repository").cloned().unwrap_or_default();
            let full_name = json_str(&repository, "full_name");
            let title = if full_name.is_empty() {
                name
            } else {
                format!("{full_name} · {name}")
            };
            let code = self.extract_code(item).join("\n");
            results.push(
                ResultBuilder::new(url, title)
                    .content(json_str(&repository, "description"))
                    .metadata("filename", json_str(item, "path"))
                    .metadata("repository", json_str(&repository, "html_url"))
                    .metadata("code", code)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Remove the leading and trailing newline bytes (LF is the byte 10) of a
/// fragment, the lstrip/rstrip pair of upstream.
fn strip_newlines(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && bytes[start] == 10 {
        start += 1;
    }
    while end > start && bytes[end - 1] == 10 {
        end -= 1;
    }
    value[start..end].to_string()
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"items":[
      {"name":"main.rs","path":"src/main.rs",
       "html_url":"https://github.com/acme/demo/blob/main/src/main.rs",
       "repository":{"full_name":"acme/demo","html_url":"https://github.com/acme/demo",
                     "description":"A tiny demo repository"},
       "text_matches":[{"object_type":"FileContent","property":"content",
                        "fragment":"fn main() {\n    run();\n}\n"}]},
      {"name":"no_url.rs","text_matches":[]}
    ]}"##;

    fn engine() -> Arc<GitHubCode> {
        GitHubCode::from_config(&EngineConfig::new(
            "github code",
            "github_code",
            "ghc",
            &["code"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_code_matches() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("run", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.title, "acme/demo · main.rs");
        assert_eq!(
            result.url,
            "https://github.com/acme/demo/blob/main/src/main.rs"
        );
        assert_eq!(result.content, "A tiny demo repository");
        assert_eq!(
            result.metadata.get("filename").map(String::as_str),
            Some("src/main.rs")
        );
        assert_eq!(
            result.metadata.get("code").map(String::as_str),
            Some("fn main() {\n    run();\n}")
        );
    }

    #[test]
    fn builds_the_api_request() {
        let query = Query::parse("run", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://api.github.com/search/code?sort=indexed&q=run&page=2"
        );
        let headers = &request.headers;
        assert_eq!(
            headers.get("accept").unwrap().to_str().unwrap(),
            ACCEPT_HEADER
        );
        assert_eq!(
            headers
                .get("x-github-api-version")
                .unwrap()
                .to_str()
                .unwrap(),
            API_VERSION
        );
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "placeholder"
        );
    }

    #[test]
    fn bearer_auth_sends_the_token() {
        let mut config = EngineConfig::new("github code", "github_code", "ghc", &["code"]);
        config.api_key = Some("secret".to_string());
        config.extra.insert(
            "ghc_auth_type".to_string(),
            toml::Value::String("bearer".to_string()),
        );
        let query = Query::parse("run", &crate::engine::EngineRegistry::default(), &[]);
        let request = GitHubCode::from_config(&config)
            .unwrap()
            .build_request(&query, 1)
            .unwrap();
        assert_eq!(
            request
                .headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer secret"
        );
    }
}
