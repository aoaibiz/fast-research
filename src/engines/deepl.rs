//! Deepl engine, ported from searx/engines/deepl.py.
// engine-id: deepl

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "translate"],
    shortcut: "dpl",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api-free.deepl.com/v2/translate";

/// DeepL, the official translation API. Upstream is an online_dictionary
/// engine, so the optional from-to prefix of the query is stripped here, like
/// the online_dictionary processor does before the engine sees the query.
pub struct Deepl {
    meta: EngineMeta,
}

impl Deepl {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn endpoint(&self) -> &str {
        self.meta.base_url.as_deref().unwrap_or(BASE_URL)
    }

    fn api_key(&self) -> Result<&str, EngineError> {
        self.meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("api_key"))
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| EngineError::Message("deepl requires an api_key".into()))
    }
}

fn is_language_code(value: &str) -> bool {
    (2..=3).contains(&value.len()) && value.chars().all(|c| c.is_ascii_alphabetic())
}

/// The DeepL target language and the text to translate.
///
/// Upstream reads both from the online_dictionary request params: the to_lang
/// code and the query without the prefix. Without a prefix the engine language
/// is the target, like the other translation engines of this crate.
fn target_and_text(query: &Query) -> (String, String) {
    let source = query.query_text();
    let mut parts = source.splitn(2, ' ');
    let first = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default().trim();
    if !rest.is_empty() {
        let mut codes = first.split('-');
        if let (Some(from), Some(to), None) = (codes.next(), codes.next(), codes.next()) {
            if is_language_code(from) && is_language_code(to) {
                return (to.to_lowercase(), rest.to_string());
            }
        }
    }
    let target = locale_language(&query.engine_language());
    let target = if target == "all" {
        "en".to_string()
    } else {
        target
    };
    (target, source)
}

impl Engine for Deepl {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.api_key()?;
        let (target, source) = target_and_text(query);
        let form: Vec<(String, String)> = vec![
            ("auth_key".to_string(), key.to_string()),
            ("text".to_string(), source),
            ("target_lang".to_string(), target),
        ];
        Ok(EngineRequest::post(self.endpoint())
            .header("Accept", "application/json")
            .form(form))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let translations = match json_path(&json, "translations").and_then(|value| value.as_array())
        {
            Some(translations) => translations,
            None => return Ok(Vec::new()),
        };
        let mut items = Vec::new();
        for translation in translations {
            let translated = json_str(translation, "text");
            if translated.trim().is_empty() {
                continue;
            }
            items.push(EngineItem::Answer(translated));
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(key: Option<&str>) -> Arc<Deepl> {
        let mut config = EngineConfig::new("deepl", "deepl", "dpl", &["general", "translate"]);
        config.api_key = key.map(str::to_string);
        Deepl::from_config(&config).expect("deepl engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    fn form_value(request: &EngineRequest, key: &str) -> String {
        request
            .form
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }

    #[test]
    fn builds_the_translation_form() {
        let request = engine(Some("KEY"))
            .build_request(&query("en-de hello world"), 1)
            .expect("request");
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, BASE_URL);
        assert_eq!(form_value(&request, "auth_key"), "KEY");
        assert_eq!(form_value(&request, "text"), "hello world");
        assert_eq!(form_value(&request, "target_lang"), "de");
    }

    #[test]
    fn falls_back_to_the_engine_language() {
        let request = engine(Some("KEY"))
            .build_request(&query("hello"), 1)
            .expect("request");
        assert_eq!(form_value(&request, "target_lang"), "en");
        assert_eq!(form_value(&request, "text"), "hello");
        let mut japanese = query("hello");
        japanese.locale = "ja-JP".to_string();
        let request = engine(Some("KEY"))
            .build_request(&japanese, 1)
            .expect("request");
        assert_eq!(form_value(&request, "target_lang"), "ja");
    }

    #[test]
    fn a_key_is_required() {
        assert!(matches!(
            engine(None).build_request(&query("hello"), 1),
            Err(EngineError::Message(_))
        ));
    }

    #[test]
    fn parses_the_translations() {
        let body = r##"{"translations":[{"detected_source_language":"EN","text":"Hallo"}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine(Some("KEY"))
            .parse(&response, &query("hello"))
            .expect("items");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Answer(answer) => assert_eq!(answer, "Hallo"),
            _ => panic!("expected an answer"),
        }
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine(Some("KEY")).parse(&empty, &query("hello"));
        assert!(items.expect("items").is_empty());
    }
}
