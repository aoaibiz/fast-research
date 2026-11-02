//! Discourse engine, ported from `searx/engines/discourse.py`.
//!
//! Searches a Discourse forum through its JSON search API. The forum's
//! `base_url` is required; `api_order`, `search_endpoint`, `show_avatar`,
//! `api_key` and `api_username` are optional engine options.
// engine-id: discourse

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "social media"],
    shortcut: "",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// The forum base URL; the engine is unusable without it.
fn base_url(meta: &EngineMeta) -> Result<String, EngineError> {
    meta.base_url
        .clone()
        .map(|value| value.trim_end_matches('/').to_string())
        .ok_or_else(|| EngineError::Message("discourse requires a base_url setting".into()))
}

/// `page` is 1-based upstream but Discourse expects the 0-based `page`.
const AGO_DAYS: &[(&str, i64)] = &[("day", 1), ("week", 7), ("month", 31), ("year", 365)];

/// Discourse forum search.
pub struct Discourse {
    meta: EngineMeta,
}

impl Discourse {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn api_order(&self) -> String {
        self.meta
            .opt_str("api_order")
            .unwrap_or("likes")
            .to_string()
    }

    fn search_endpoint(&self) -> &str {
        self.meta
            .opt_str("search_endpoint")
            .unwrap_or("/search.json")
    }

    fn api_username(&self) -> String {
        self.meta.opt_str("api_username").unwrap_or("").to_string()
    }
}

impl Engine for Discourse {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = base_url(&self.meta)?;
        let mut q = format!("{} order:{}", query.query_text(), self.api_order());
        if let Some(range) = query.time_range {
            let days = AGO_DAYS
                .iter()
                .find(|(name, _)| *name == range.as_str())
                .map(|(_, days)| *days)
                .unwrap_or(1);
            let after = chrono::Utc::now() - chrono::Duration::days(days);
            q = format!("{q} after:{}", after.format("%Y-%m-%d"));
        }
        let pairs = [("q", q), ("page", pageno.saturating_sub(1).to_string())];
        let mut request = EngineRequest::get(format!(
            "{base}{}?{}",
            self.search_endpoint(),
            query_string(&pairs)
        ))
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("X-Requested-With", "XMLHttpRequest");
        if let Some(api_key) = &self.meta.api_key {
            if !api_key.is_empty() {
                request = request.header("Api-Key", api_key);
            }
        }
        let username = self.api_username();
        if !username.is_empty() {
            request = request.header("Api-Username", &username);
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        // Upstream returns empty when either container is missing.
        let Some(topics) = json_path(&json, "topics").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let Some(posts) = json_path(&json, "posts").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let base = match base_url(&self.meta) {
            Ok(base) => base,
            Err(_) => return Ok(Vec::new()),
        };
        let show_avatar = self.meta.opt_bool("show_avatar").unwrap_or(false);
        // Build a topic id -> topic map.
        let mut topics_by_id = std::collections::HashMap::new();
        for topic in topics {
            let id = json_str(topic, "id");
            if !id.is_empty() {
                topics_by_id.insert(id, topic);
            }
        }
        let mut results = Vec::new();
        for post in posts {
            let post_id = json_str(post, "id");
            let topic_id = json_str(post, "topic_id");
            let Some(topic) = topics_by_id.get(&topic_id) else {
                continue;
            };
            let url = format!("{base}/p/{post_id}");
            let title = decode_entities(&json_str(topic, "title"));
            if url.is_empty() || title.is_empty() {
                continue;
            }
            let content = decode_entities(&json_str(post, "blurb"));
            let comments = json_str(topic, "posts_count");
            let mut metadata = vec![format!("@{}", json_str(post, "username"))];
            let comments_int = comments.parse::<i64>().unwrap_or(0);
            let closed = json_str(topic, "closed") == "true";
            if comments_int > 1 {
                metadata.push(format!("comments: {comments}"));
            }
            if json_str(topic, "has_accepted_answer") == "true" {
                metadata.push("answered".to_string());
            } else if comments_int > 1 {
                metadata.push(if closed {
                    "closed".to_string()
                } else {
                    "open".to_string()
                });
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(content)
                .published_date(json_str(topic, "created_at"))
                .metadata("metadata", metadata.join(" | "));
            if show_avatar {
                let avatar = json_str(post, "avatar_template").replace("{size}", "96");
                if !avatar.is_empty() {
                    builder = builder.thumbnail(format!("{base}{avatar}"));
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_topics_and_posts() {
        let fixture = r#"{
          "topics": [
            {"id": "1", "title": "&amp; Hello Rust", "posts_count": 3,
             "created_at": "2024-01-01T10:00:00Z", "closed": false,
             "has_accepted_answer": true},
            {"id": "2", "title": "Unrelated", "posts_count": 1,
             "created_at": "2024-01-02T10:00:00Z", "closed": true,
             "has_accepted_answer": false}
          ],
          "posts": [
            {"id": "11", "topic_id": "1", "username": "alice",
             "blurb": "A &lt;b&gt;question&lt;/b&gt;", "avatar_template": "/a/{size}.png"}
          ]
        }"#;
        let mut config = EngineConfig::new("discourse", "discourse", "", &["social media"]);
        config.base_url = Some("https://forums.example.com/".to_string());
        let engine = Discourse::from_config(&config).unwrap();
        let response = EngineResponse::new(
            200,
            "https://forums.example.com",
            fixture.as_bytes().to_vec(),
        );
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://forums.example.com/p/11");
                assert_eq!(result.title, "& Hello Rust");
                assert!(result.content.contains("question"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn build_request_requires_a_base_url() {
        let engine = Discourse::from_config(&EngineConfig::new(
            "discourse",
            "discourse",
            "",
            &["general"],
        ))
        .unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine.build_request(&query, 1),
            Err(EngineError::Message(_))
        ));
    }

    #[test]
    fn build_request_sets_the_search_endpoint() {
        let mut config = EngineConfig::new("discourse", "discourse", "", &["general"]);
        config.base_url = Some("https://forums.example.com/".to_string());
        let engine = Discourse::from_config(&config).unwrap();
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://forums.example.com/search.json")
        );
        assert!(request.url.contains("page=1"));
        // the query string is percent-encoded, "order:likes" travels as order%3Alikes
        assert!(request.url.contains("order%3Alikes") || request.url.contains("order:likes"));
    }
}
