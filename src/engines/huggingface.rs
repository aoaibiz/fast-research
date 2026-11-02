//! Hugging Face engine, ported from searx/engines/huggingface.py.
//!
//! Models, datasets and spaces are served by the same API; the endpoint is
//! selected with the huggingface_endpoint setting.
// engine-id: huggingface

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it", "repos"],
    shortcut: "hf",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://huggingface.co";

/// Hugging Face model, dataset and space search.
pub struct HuggingFace {
    meta: EngineMeta,
}

impl HuggingFace {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let endpoint = config.opt_str("huggingface_endpoint").unwrap_or("models");
        if !matches!(endpoint, "datasets" | "models" | "spaces") {
            return Err(format!("unsupported Hugging Face endpoint: {endpoint}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }

    fn endpoint(&self) -> String {
        self.meta
            .opt_str("huggingface_endpoint")
            .unwrap_or("models")
            .to_string()
    }
}

impl Engine for HuggingFace {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("direction", "-1".to_string()),
            ("search", query.query_text()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/api/{}?{}",
            self.base_url(),
            self.endpoint(),
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
        let Some(entries) = json.as_array() else {
            return Ok(Vec::new());
        };
        let base = self.base_url();
        let endpoint = self.endpoint();
        let mut results = Vec::new();
        for entry in entries {
            let id = json_str(entry, "id");
            if id.is_empty() {
                continue;
            }
            let url = if endpoint == "models" {
                format!("{base}/{id}")
            } else {
                format!("{base}/{endpoint}/{id}")
            };
            if !is_http_url(&url) {
                continue;
            }
            let mut parts = Vec::new();
            if let Some(likes) = count(entry, "likes") {
                parts.push(format!("Likes: {likes}"));
            }
            if let Some(downloads) = count(entry, "downloads") {
                parts.push(format!("Downloads: {}", thousands(downloads)));
            }
            let tags = tag_list(entry);
            if !tags.is_empty() {
                parts.push(format!("Tags: {}", tags.join(", ")));
            }
            let description = json_str(entry, "description");
            if !description.is_empty() {
                parts.push(format!("Description: {description}"));
            }
            results.push(
                ResultBuilder::new(url, id)
                    .content(strip_tags(&parts.join(" | ")))
                    .published_date(date(&json_str(entry, "createdAt")).unwrap_or_default())
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// A non zero numeric field, upstream tests the truthiness of the value.
fn count(entry: &serde_json::Value, key: &str) -> Option<i64> {
    json_path(entry, key)
        .and_then(serde_json::Value::as_i64)
        .filter(|value| *value != 0)
}

/// Thousands separators, like the format spec of upstream.
fn thousands(value: i64) -> String {
    let digits = value.max(0).to_string();
    let mut output = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn tag_list(entry: &serde_json::Value) -> Vec<String> {
    json_path(entry, "tags")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Upstream drops the Z suffix and parses the rest with fromisoformat.
fn date(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('Z');
    if let Ok(value) = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(value.format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .map(|value| value.format("%Y-%m-%dT00:00:00").to_string())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"[
      {"id":"bert-base-uncased","likes":12,"downloads":1234567,"tags":["transformers","pytorch"],"description":"A <b>BERT</b> model.","createdAt":"2022-12-01T00:00:00.000Z"},
      {"id":"gpt2","createdAt":"not a date"},
      {"id":""}
    ]"##;

    fn engine() -> Arc<HuggingFace> {
        HuggingFace::from_config(&EngineConfig::new(
            "huggingface",
            "huggingface",
            "hf",
            &["it", "repos"],
        ))
        .unwrap()
    }

    fn engine_with(endpoint: &str) -> Arc<HuggingFace> {
        let mut config = EngineConfig::new("huggingface", "huggingface", "hfd", &["it"]);
        config.extra.insert(
            "huggingface_endpoint".into(),
            toml::Value::String(endpoint.into()),
        );
        HuggingFace::from_config(&config).unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn response(body: &str) -> EngineResponse {
        EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec())
    }

    #[test]
    fn parses_models() {
        let items = engine().parse(&response(FIXTURE), &query("bert")).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://huggingface.co/bert-base-uncased");
        assert_eq!(result.title, "bert-base-uncased");
        assert_eq!(
            result.content,
            "Likes: 12 | Downloads: 1,234,567 | Tags: transformers, pytorch | Description: A BERT model."
        );
        assert_eq!(
            result.published_date.as_deref(),
            Some("2022-12-01T00:00:00")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(second.url, "https://huggingface.co/gpt2");
        assert_eq!(second.published_date, None);
        assert_eq!(second.content, "");
    }

    #[test]
    fn builds_dataset_urls_and_requests() {
        let items = engine_with("datasets")
            .parse(&response(r##"[{"id":"squad"}]"##), &query("squad"))
            .unwrap();
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://huggingface.co/datasets/squad");
        let request = engine_with("datasets")
            .build_request(&query("rust"), 1)
            .unwrap();
        assert_eq!(
            request.url,
            "https://huggingface.co/api/datasets?direction=-1&search=rust"
        );
    }

    #[test]
    fn rejects_unknown_endpoints() {
        let mut config = EngineConfig::new("huggingface", "huggingface", "hf", &["it"]);
        config.extra.insert(
            "huggingface_endpoint".into(),
            toml::Value::String("papers".into()),
        );
        assert!(HuggingFace::from_config(&config).is_err());
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let body = r##"{"error":"bad"}"##;
        assert!(
            engine()
                .parse(&response(body), &query("bert"))
                .unwrap()
                .is_empty()
        );
    }
}
