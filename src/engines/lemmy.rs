//! Lemmy engine, ported from `searx/engines/lemmy.py`.
//!
//! The API of a (federated) Lemmy instance is searched; the kind of entry is
//! selected with the `lemmy_type` option (`Communities`, `Users`, `Posts`,
//! `Comments`).
// engine-id: lemmy

use std::sync::Arc;

use regex::Regex;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "leco",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://lemmy.ml";

/// A Lemmy instance.
pub struct Lemmy {
    meta: EngineMeta,
}

impl Lemmy {
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

    fn lemmy_type(&self) -> &str {
        self.meta.opt_str("lemmy_type").unwrap_or("Communities")
    }
}

impl Engine for Lemmy {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("type_", self.lemmy_type().to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/api/v3/search?{}",
            self.base_url(),
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
        let mut results = Vec::new();
        match self.lemmy_type() {
            "Communities" => {
                for item in array(&json, "communities") {
                    let url = json_str(item, "community.actor_id");
                    let title = json_str(item, "community.title");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    let community = json_path(item, "community");
                    let thumbnail = if community
                        .map(|value| value.get("icon").is_some())
                        .unwrap_or(false)
                    {
                        json_str(item, "community.icon")
                    } else {
                        json_str(item, "community.banner")
                    };
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(markdown_to_text(&json_str(item, "community.description")))
                            .thumbnail(thumbnail)
                            .published_date(iso_date(&json_str(item, "counts.published")))
                            .metadata("subscribers", json_str(item, "counts.subscribers"))
                            .metadata("posts", json_str(item, "counts.posts"))
                            .metadata(
                                "active users",
                                json_str(item, "counts.users_active_half_year"),
                            )
                            .build(),
                    );
                }
            }
            "Users" => {
                for item in array(&json, "users") {
                    let url = json_str(item, "person.actor_id");
                    let title = json_str(item, "person.name");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(markdown_to_text(&json_str(item, "person.bio")))
                            .build(),
                    );
                }
            }
            "Posts" => {
                for item in array(&json, "posts") {
                    let url = json_str(item, "post.ap_id");
                    let title = json_str(item, "post.name");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    let user = {
                        let display_name = json_str(item, "creator.display_name");
                        if display_name.is_empty() {
                            json_str(item, "creator.name")
                        } else {
                            display_name
                        }
                    };
                    let thumbnail = {
                        let thumbnail_url = json_str(item, "post.thumbnail_url");
                        if thumbnail_url.is_empty() {
                            String::new()
                        } else {
                            format!("{thumbnail_url}?format=webp&thumbnail=208")
                        }
                    };
                    let body = markdown_to_text(&json_str(item, "post.body"));
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(body)
                            .thumbnail(thumbnail)
                            .published_date(iso_date(&json_str(item, "post.published")))
                            .metadata("upvotes", json_str(item, "counts.upvotes"))
                            .metadata("downvotes", json_str(item, "counts.downvotes"))
                            .metadata("user", user)
                            .metadata("comments", json_str(item, "counts.comments"))
                            .metadata("community", json_str(item, "community.title"))
                            .build(),
                    );
                }
            }
            "Comments" => {
                for item in array(&json, "comments") {
                    let url = json_str(item, "comment.ap_id");
                    let title = json_str(item, "post.name");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    let user = {
                        let display_name = json_str(item, "creator.display_name");
                        if display_name.is_empty() {
                            json_str(item, "creator.name")
                        } else {
                            display_name
                        }
                    };
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(markdown_to_text(&json_str(item, "comment.content")))
                            .published_date(iso_date(&json_str(item, "comment.published")))
                            .metadata("upvotes", json_str(item, "counts.upvotes"))
                            .metadata("downvotes", json_str(item, "counts.downvotes"))
                            .metadata("user", user)
                            .metadata("community", json_str(item, "community.title"))
                            .build(),
                    );
                }
            }
            other => {
                return Err(EngineError::Message(format!(
                    "lemmy: unsupported lemmy_type {other:?}"
                )));
            }
        }
        Ok(results)
    }
}

fn array<'a>(value: &'a serde_json::Value, path: &str) -> &'a [serde_json::Value] {
    json_path(value, path)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream truncates the ISO timestamp to seconds before parsing it.
fn iso_date(value: &str) -> String {
    let truncated: String = value.trim().chars().take(19).collect();
    if truncated.len() >= 10 {
        truncated
    } else {
        String::new()
    }
}

/// A pragmatic port of upstream's `markdown_to_text` (markdown-it + html_to_text).
fn markdown_to_text(value: &str) -> String {
    let mut text = value.to_string();
    for (pattern, replacement) in [
        (r"!\[([^\]]*)\]\([^)]*\)", "$1"),
        (r"\[([^\]]*)\]\([^)]*\)", "$1"),
        (r"(?m)^\s{0,3}(?:#{1,6}\s*|>\s?|[-*+]\s+|\d+\.\s+)", ""),
    ] {
        if let Ok(regex) = Regex::new(pattern) {
            text = regex.replace_all(&text, replacement).to_string();
        }
    }
    text = text
        .replace("```", " ")
        .replace('`', "")
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace('*', "");
    strip_tags(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "communities": [
        {
          "community": {
            "actor_id": "https://lemmy.ml/c/rust",
            "title": "Rust",
            "description": "A place for **Rust** news.",
            "icon": "https://lemmy.ml/pictrs/image/rust.png"
          },
          "counts": {
            "subscribers": 1234,
            "posts": 42,
            "users_active_half_year": 99,
            "published": "2021-01-01T00:00:00.000000Z"
          }
        }
      ]
    }"#;

    fn engine() -> Arc<Lemmy> {
        Lemmy::from_config(&EngineConfig::new(
            "lemmy communities",
            "lemmy",
            "leco",
            &["social media"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_communities() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust");
                assert_eq!(result.url, "https://lemmy.ml/c/rust");
                assert_eq!(result.content, "A place for Rust news.");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://lemmy.ml/pictrs/image/rust.png")
                );
                assert_eq!(
                    result.metadata.get("subscribers").map(String::as_str),
                    Some("1234")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "https://lemmy.ml/api/v3/search?q=rust&page=2&type_=Communities"
        );
    }
}
