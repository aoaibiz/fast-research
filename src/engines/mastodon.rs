//! Mastodon engine, ported from `searx/engines/mastodon.py`.
//!
//! The search API of a Mastodon instance; `mastodon_type` selects between
//! `accounts` and `hashtags`.  The Search API forbids pagination without
//! OAuth, so upstream (and this port) do not page.
// engine-id: mastodon

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["social media"],
    shortcut: "mau",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://mastodon.social";
/// `page_size`, see searxng/searxng#2857.
const PAGE_SIZE: i64 = 40;

/// A Mastodon instance.
pub struct Mastodon {
    meta: EngineMeta,
}

impl Mastodon {
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

    fn mastodon_type(&self) -> &str {
        self.meta.opt_str("mastodon_type").unwrap_or("accounts")
    }
}

impl Engine for Mastodon {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("resolve", "false".to_string()),
            ("type", self.mastodon_type().to_string()),
            (
                "limit",
                self.meta
                    .opt_i64("page_size")
                    .unwrap_or(PAGE_SIZE)
                    .to_string(),
            ),
        ];
        Ok(EngineRequest::get(format!(
            "{}/api/v2/search?{}",
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
        let mastodon_type = self.mastodon_type();
        let Some(items) = json_path(&json, mastodon_type).and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        match mastodon_type {
            "accounts" => {
                for item in items {
                    let url = json_str(item, "uri");
                    let title = json_str(item, "username");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    let followers = json_str(item, "followers_count");
                    results.push(
                        ResultBuilder::new(url, format!("{title} ({followers} followers)"))
                            .content(json_str(item, "note"))
                            .thumbnail(json_str(item, "avatar"))
                            .published_date(date_only(&json_str(item, "created_at")))
                            .build(),
                    );
                }
            }
            "hashtags" => {
                for item in items {
                    let url = json_str(item, "url");
                    let title = json_str(item, "name");
                    if !is_http_url(&url) || title.is_empty() {
                        continue;
                    }
                    let mut uses_count: i64 = 0;
                    let mut user_count: i64 = 0;
                    if let Some(history) =
                        json_path(item, "history").and_then(serde_json::Value::as_array)
                    {
                        for entry in history {
                            uses_count += json_path(entry, "uses")
                                .and_then(serde_json::Value::as_i64)
                                .or_else(|| json_str(entry, "uses").parse().ok())
                                .unwrap_or(0);
                            user_count += json_path(entry, "accounts")
                                .and_then(serde_json::Value::as_i64)
                                .or_else(|| json_str(entry, "accounts").parse().ok())
                                .unwrap_or(0);
                        }
                    }
                    results.push(
                        ResultBuilder::new(url, title)
                            .content(format!(
                                "Hashtag has been used {uses_count} times by {user_count} different users"
                            ))
                            .build(),
                    );
                }
            }
            other => {
                return Err(EngineError::Message(format!(
                    "mastodon: unsupported mastodon_type {other:?}"
                )));
            }
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream keeps only the date part of `created_at`.
fn date_only(value: &str) -> String {
    value.trim().chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "accounts": [
        {
          "uri": "https://mastodon.social/@searxng",
          "username": "searxng",
          "followers_count": 1200,
          "note": "<p>A privacy friendly search engine.</p>",
          "avatar": "https://mastodon.social/avatar.png",
          "created_at": "2021-01-01T00:00:00.000Z"
        }
      ]
    }"#;

    fn engine() -> Arc<Mastodon> {
        Mastodon::from_config(&EngineConfig::new(
            "mastodon users",
            "mastodon",
            "mau",
            &["social media"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_accounts() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("searxng", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "searxng (1200 followers)");
                assert_eq!(result.url, "https://mastodon.social/@searxng");
                assert_eq!(result.published_date.as_deref(), Some("2021-01-01"));
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://mastodon.social/avatar.png")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let query = Query::parse("searxng", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "https://mastodon.social/api/v2/search?q=searxng&resolve=false&type=accounts&limit=40"
        );
    }
}
