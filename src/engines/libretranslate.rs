//! LibreTranslate engine, ported from searx/engines/libretranslate.py.
//!
//! Upstream's processor strips an en-de prefix and passes the language pair;
//! this port parses the prefix itself and otherwise targets the engine language.
// engine-id: libretranslate

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "translate"],
    shortcut: "lt",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://libretranslate.com/translate";
const ALTERNATIVES: u32 = 3;

/// LibreTranslate, the free machine translation API.
pub struct LibreTranslate {
    meta: EngineMeta,
}

impl LibreTranslate {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

/// The endpoint. Upstream appends /translate, but the shipped base_url already
/// ends with it, so this port only appends the path when it is missing.
fn endpoint(meta: &EngineMeta) -> String {
    let base = meta
        .base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/');
    if base.ends_with("/translate") {
        base.to_string()
    } else {
        format!("{base}/translate")
    }
}

fn is_language_code(value: &str) -> bool {
    (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphabetic())
}

/// Split the optional from-to prefix off the query, like the upstream
/// online_dictionary processor.
fn direction(query: &Query) -> (String, String, String) {
    let text = query.query_text();
    let mut parts = text.splitn(2, ' ');
    let first = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default().trim();
    if !rest.is_empty() {
        let mut codes = first.split('-');
        if let (Some(from), Some(to), None) = (codes.next(), codes.next(), codes.next()) {
            if is_language_code(from) && is_language_code(to) {
                return (from.to_lowercase(), to.to_lowercase(), rest.to_string());
            }
        }
    }
    let target = locale_language(&query.engine_language());
    let target = if target == "all" {
        "en".to_string()
    } else {
        target
    };
    ("auto".to_string(), target, text)
}

impl Engine for LibreTranslate {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let url = endpoint(&self.meta);
        let api_key = self.meta.api_key.as_deref().unwrap_or_default();
        if url.starts_with("https://libretranslate.com") && api_key.is_empty() {
            return Err(EngineError::Message(
                "libretranslate.com requires an api_key; configure a mirror in base_url instead"
                    .into(),
            ));
        }
        let (source, target, text) = direction(query);
        let mut body = serde_json::json!({
            "q": text,
            "source": source,
            "target": target,
            "alternatives": ALTERNATIVES,
        });
        if !api_key.is_empty() {
            body["api_key"] = serde_json::Value::String(api_key.to_string());
        }
        Ok(EngineRequest::post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json_body(body))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let translated = json_str(&json, "translatedText");
        if translated.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut items = vec![EngineItem::Answer(translated)];
        let alternatives: Vec<&str> = json_path(&json, "alternatives")
            .and_then(|value| value.as_array())
            .map(|values| values.iter().filter_map(|value| value.as_str()).collect())
            .unwrap_or_default();
        if !alternatives.is_empty() {
            items.push(EngineItem::EngineData {
                key: "alternatives".to_string(),
                value: alternatives.join(", "),
            });
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(base: Option<&str>, key: Option<&str>) -> Arc<LibreTranslate> {
        let mut config = EngineConfig::new(
            "libretranslate",
            "libretranslate",
            "lt",
            &["general", "translate"],
        );
        config.base_url = base.map(str::to_string);
        config.api_key = key.map(str::to_string);
        LibreTranslate::from_config(&config).expect("libretranslate engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_a_json_request_for_a_mirror() {
        let default = engine(None, None);
        assert!(default.build_request(&query("hello"), 1).is_err());
        let mirror = engine(Some("https://translate.example.com"), Some("secret"));
        assert_eq!(
            endpoint(&mirror.meta),
            "https://translate.example.com/translate"
        );
        let request = mirror
            .build_request(&query("en-de hello world"), 1)
            .unwrap();
        assert_eq!(request.url, "https://translate.example.com/translate");
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["q"].as_str(), Some("hello world"));
        assert_eq!(body["source"].as_str(), Some("en"));
        assert_eq!(body["target"].as_str(), Some("de"));
        assert_eq!(body["alternatives"].as_u64(), Some(3));
        assert_eq!(body["api_key"].as_str(), Some("secret"));
        let request = engine(Some("https://translate.example.com"), None)
            .build_request(&query(":ja こんにちは"), 0)
            .unwrap();
        let body = request.json.as_ref().expect("json body");
        assert_eq!(body["q"].as_str(), Some("こんにちは"));
        assert_eq!(body["source"].as_str(), Some("auto"));
        assert_eq!(body["target"].as_str(), Some("ja"));
        assert!(body.get("api_key").is_none());
    }

    #[test]
    fn parses_the_translation_and_its_alternatives() {
        let body = r##"{"translatedText":"Hallo","alternatives":["Servus","Guten Tag"]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine(None, None)
            .parse(&response, &query("hello"))
            .unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Answer(answer) => assert_eq!(answer, "Hallo"),
            _ => panic!("expected an answer"),
        }
        match &items[1] {
            EngineItem::EngineData { key, value } => {
                assert_eq!(key, "alternatives");
                assert_eq!(value, "Servus, Guten Tag");
            }
            _ => panic!("expected engine data"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            engine(None, None)
                .parse(&empty, &query("hello"))
                .unwrap()
                .is_empty()
        );
    }
}
