//! Cloudflare AI engine, ported from `searx/engines/cloudflareai.py`.

// engine-id: cloudflareai

use std::sync::Arc;

use crate::engine::util::json_path;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "cfai",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 30.0,
};

const DEFAULT_ASSISTANT: &str = "Keep your answers as short and effective as possible.";
const DEFAULT_SYSTEM: &str = "You are a self-aware language model who is honest and direct about any question from the user.";

/// Cloudflare Workers AI chat engine. Requires account/gateway/model config.
pub struct CloudflareAi {
    meta: EngineMeta,
}

impl CloudflareAi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for CloudflareAi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let account = self.meta.opt_str("cf_account_id").unwrap_or("");
        let gateway = self.meta.opt_str("cf_ai_gateway").unwrap_or("");
        let model = self.meta.opt_str("cf_ai_model").unwrap_or("");
        let api_key = self.meta.opt_str("cf_ai_api").unwrap_or("");
        if account.is_empty() || gateway.is_empty() || model.is_empty() {
            return Err(EngineError::Message(
                "cloudflareai requires cf_account_id, cf_ai_gateway and cf_ai_model".into(),
            ));
        }
        let assistant = self
            .meta
            .opt_str("cf_ai_model_assistant")
            .unwrap_or(DEFAULT_ASSISTANT);
        let system = self
            .meta
            .opt_str("cf_ai_model_system")
            .unwrap_or(DEFAULT_SYSTEM);
        let url =
            format!("https://gateway.ai.cloudflare.com/v1/{account}/{gateway}/workers-ai/{model}");
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": assistant},
                {"role": "system", "content": system},
                {"role": "user", "content": query.query_text()},
            ]
        });
        Ok(EngineRequest::post(url)
            .json_body(body)
            .header("Authorization", &format!("Bearer {api_key}"))
            .header("Content-Type", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if let Some(error) = json.get("error") {
            return Err(EngineError::Message(format!(
                "Cloudflare AI error: {}",
                serde_json::to_string(error).unwrap_or_default()
            )));
        }
        let mut results = Vec::new();
        if let Some(answer) = json_path(&json, "result.response").and_then(|v| v.as_str()) {
            let answer = answer.trim();
            if !answer.is_empty() {
                results.push(EngineItem::Answer(answer.to_string()));
            }
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_answer() {
        let body = r#"{"result":{"response":"Hello, world!"}}"#;
        let response = EngineResponse::new(
            200,
            "https://gateway.ai.cloudflare.com/v1/a/g/w/x",
            body.as_bytes().to_vec(),
        );
        let engine = CloudflareAi::from_config(&EngineConfig::new(
            "cloudflareai",
            "cloudflareai",
            "cfai",
            &["general"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("hi", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], EngineItem::Answer(a) if a == "Hello, world!"));
    }
}
