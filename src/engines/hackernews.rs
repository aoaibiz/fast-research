//! Hacker News engine, ported from `searx/engines/hackernews.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "news"],
    shortcut: "hn",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://hn.algolia.com/api/v1";

/// Hacker News, through the Algolia API.
pub struct HackerNews {
    meta: EngineMeta,
}

impl HackerNews {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for HackerNews {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // The Algolia query tuning of upstream: without it the ranking differs
        // from the one SearXNG shows for the same query.
        let mut pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("page", (pageno.saturating_sub(1)).to_string()),
            ("hitsPerPage", "10".to_string()),
            ("minWordSizefor1Typo", "4".to_string()),
            ("minWordSizefor2Typos", "8".to_string()),
            ("advancedSyntax", "true".to_string()),
            ("ignorePlurals", "false".to_string()),
            ("minProximity", "7".to_string()),
            ("numericFilters", "[]".to_string()),
            ("tagFilters", "[\"story\",[]]".to_string()),
            ("typoTolerance", "true".to_string()),
            ("queryType", "prefixLast".to_string()),
            (
                "restrictSearchableAttributes",
                "[\"title\",\"comment_text\",\"url\",\"story_text\",\"author\"]".to_string(),
            ),
            ("getRankingInfo", "true".to_string()),
        ];
        if let Some(range) = query.time_range {
            if let Some(seconds) = crate::query::TimeRange::seconds(Some(range)) {
                let timestamp = chrono::Utc::now().timestamp() - seconds;
                // Replace the empty filter sent above.
                pairs.retain(|(key, _)| *key != "numericFilters");
                pairs.push(("numericFilters", format!("created_at_i>{timestamp}")));
            }
        }
        // \`search_by_date = true\` switches to Algolia's date ordered index (the
        // newest stories first), upstream does the same for empty queries.
        let index = if self.meta.opt_bool("search_by_date").unwrap_or(false)
            || query.query_text().is_empty()
        {
            "search_by_date"
        } else {
            "search"
        };
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/{index}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "hits").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        // Upstream links to the Hacker News discussion and keeps the article URL
        // in the content; \`link_to_item = false\` links to the article instead.
        let link_to_item = self.meta.opt_bool("link_to_item").unwrap_or(true);
        for item in items {
            let object_id = json_str(item, "objectID");
            let item_url = format!("https://news.ycombinator.com/item?id={object_id}");
            let article_url = json_str(item, "url");
            let url = if link_to_item || article_url.is_empty() {
                item_url
            } else {
                article_url.clone()
            };
            let title = {
                let title = json_str(item, "title");
                if title.is_empty() {
                    json_str(item, "story_title")
                } else {
                    title
                }
            };
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let points = json_str(item, "points");
            let comments = json_str(item, "num_comments");
            let mut builder = ResultBuilder::new(url, title)
                .author(json_str(item, "author"))
                .published_date(json_str(item, "created_at"))
                .metadata("points", points.clone())
                .metadata("comments", comments.clone());
            if link_to_item && !article_url.is_empty() {
                // The article URL is the content, exactly like upstream.
                builder = builder.content(article_url);
            } else {
                let mut content = Vec::new();
                if !points.is_empty() {
                    content.push(format!("{points} points"));
                }
                if !comments.is_empty() {
                    content.push(format!("{comments} comments"));
                }
                if !content.is_empty() {
                    builder = builder.content(content.join(", "));
                }
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}
