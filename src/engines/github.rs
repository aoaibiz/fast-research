//! GitHub engine, ported from `searx/engines/github.py`.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "repositories"],
    shortcut: "gh",
    paging: false,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://api.github.com/search/repositories";

/// GitHub repository search.
pub struct GitHub {
    meta: EngineMeta,
}

impl GitHub {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for GitHub {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // With a time range, recently pushed repositories are searched for
        // instead of the most starred ones.
        let mut text = query.query_text();
        let mut sort = "stars";
        if let Some(since) = crate::query::TimeRange::since(query.time_range) {
            text = format!("{text} pushed:>{}", since.format("%Y-%m-%d"));
            sort = "updated";
        }
        let pairs: Vec<(&str, String)> = vec![
            ("q", text),
            ("sort", sort.to_string()),
            ("order", "desc".to_string()),
            // No explicit page size upstream, so the API default (30) applies.
            ("per_page", "30".to_string()),
        ];
        let mut request = EngineRequest::get(format!("{SEARCH_URL}?{}", query_string(&pairs)))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(key) = &self.meta.api_key {
            if !key.is_empty() {
                request = request.header("Authorization", &format!("Bearer {key}"));
            }
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(message) = json.get("message").and_then(|value| value.as_str()) {
            if json.get("items").is_none() {
                return Err(EngineError::Message(format!("github api: {message}")));
            }
        }
        let items = match json_path(&json, "items").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let full_name = json_str(item, "full_name");
            let url = json_str(item, "html_url");
            if full_name.is_empty() || url.is_empty() {
                continue;
            }
            let stars = json_str(item, "stargazers_count");
            let language = json_str(item, "language");
            let license = json_str(item, "license.spdx_id");
            let description = json_str(item, "description");
            let mut content = Vec::new();
            if !description.is_empty() {
                content.push(description);
            }
            if !language.is_empty() {
                content.push(format!("Language: {language}"));
            }
            if !stars.is_empty() {
                content.push(format!("{stars} stars"));
            }
            if !license.is_empty() && license != "NOASSERTION" {
                content.push(format!("License: {license}"));
            }
            results.push(
                ResultBuilder::new(url, full_name)
                    .content(content.join(" | "))
                    .published_date(json_str(item, "updated_at"))
                    .metadata("stars", stars)
                    .metadata("language", language)
                    .build(),
            );
        }
        Ok(results)
    }
}
