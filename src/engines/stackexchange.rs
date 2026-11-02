//! Stack Exchange engine, ported from `searx/engines/stackexchange.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "q&a"],
    shortcut: "so",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://api.stackexchange.com/2.3/search/advanced";

/// Stack Exchange (Stack Overflow by default).
pub struct StackExchange {
    meta: EngineMeta,
}

impl StackExchange {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The Stack Exchange site, `stackoverflow` unless configured otherwise.
    fn site(&self) -> String {
        self.meta
            .opt_str("api_site")
            .map(str::to_string)
            .unwrap_or_else(|| "stackoverflow".to_string())
    }
}

impl Engine for StackExchange {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        // With a time range the newest questions are asked for, otherwise
        // relevance wins (upstream supports fromdate/todate too).
        let mut pairs: Vec<(&str, String)> = vec![
            ("order", "desc".to_string()),
            (
                "sort",
                if query.time_range.is_some() {
                    "creation".to_string()
                } else {
                    // upstream default api_sort is "activity"
                    self.meta
                        .opt_str("api_sort")
                        .unwrap_or("activity")
                        .to_string()
                },
            ),
            ("q", query.query_text()),
            ("site", self.site()),
            ("pagesize", "10".to_string()),
            ("page", pageno.to_string()),
        ];
        if let Some(seconds) = crate::query::TimeRange::seconds(query.time_range) {
            let from = chrono::Utc::now().timestamp().saturating_sub(seconds);
            pairs.push(("fromdate", from.to_string()));
        }
        Ok(
            EngineRequest::get(format!("{SEARCH_API}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(message) = json.get("error_message").and_then(|value| value.as_str()) {
            return Err(EngineError::Message(format!(
                "stack exchange api: {message}"
            )));
        }
        let items = match json_path(&json, "items").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = strip_tags(&json_str(item, "title"));
            // upstream builds the short canonical URL from the question id
            let question_id = json_str(item, "question_id");
            let mut url = json_str(item, "link");
            if !question_id.is_empty() {
                url = format!("https://{}.com/q/{question_id}", self.site());
            }
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let score = json_str(item, "score");
            let answers = json_str(item, "answer_count");
            let answered = item
                .get("is_answered")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let mut content = Vec::new();
            if answered && !answers.is_empty() {
                content.push(format!("{answers} answers"));
            }
            if !score.is_empty() {
                content.push(format!("score {score}"));
            }
            let owner = json_str(item, "owner.display_name");
            results.push(
                ResultBuilder::new(url, title)
                    .content(content.join(", "))
                    .author(owner)
                    .metadata("score", score)
                    .metadata("answers", answers)
                    .build(),
            );
        }
        Ok(results)
    }
}
