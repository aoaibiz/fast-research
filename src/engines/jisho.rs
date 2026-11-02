//! Jisho engine, ported from searx/engines/jisho.py.
//!
//! Each JSON entry becomes a result; the first entry also yields upstream's infobox.
// engine-id: jisho

use std::sync::Arc;

use serde_json::Value;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::{Infobox, InfoboxUrl};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["dictionaries"],
    shortcut: "js",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

const SITE: &str = "https://jisho.org";

/// Jisho, the Japanese-English dictionary.
pub struct Jisho {
    meta: EngineMeta,
}

impl Jisho {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn site(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(SITE)
        .trim_end_matches('/')
        .to_string()
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase()),
        None => String::new(),
    }
}
fn alternative_forms(page: &Value) -> Vec<String> {
    let Some(entries) = page.get("japanese").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut forms = Vec::new();
    for entry in entries {
        let word = entry
            .get("word")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reading = entry
            .get("reading")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !word.is_empty() {
            forms.push(if reading.is_empty() {
                word.to_string()
            } else {
                format!("{word} ({reading})")
            });
        } else if !reading.is_empty() {
            forms.push(reading.to_string());
        }
    }
    forms
}
fn sense_definitions(page: &Value) -> Vec<(String, String, String)> {
    let Some(senses) = page.get("senses").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut definitions = Vec::new();
    for sense in senses {
        let tags = strings(sense, "tags");
        let info = strings(sense, "info");
        let mut extra = if tags.is_empty() {
            if info.is_empty() {
                String::new()
            } else {
                format!("{}. ", capitalize(&info.join(", ")))
            }
        } else if info.is_empty() {
            format!("{}. ", tags.join(", "))
        } else {
            format!("{}, {}. ", tags[0], info[0])
        };
        let restrictions = strings(sense, "restrictions");
        if !restrictions.is_empty() {
            extra.push_str(&format!("Only applies to: {}. ", restrictions.join(", ")));
        }
        if extra.ends_with(' ') {
            extra.pop();
        }
        definitions.push((
            strings(sense, "parts_of_speech").join(", "),
            strings(sense, "english_definitions").join("; "),
            extra,
        ));
    }
    definitions
}
fn infobox(forms: &[String], url: &str, definitions: &[(String, String, String)]) -> Infobox {
    let mut content = String::new();
    if forms.len() > 1 {
        content.push_str(&format!(
            "<p><i>Other forms:</i> {}</p>",
            forms[1..].join(", ")
        ));
    }
    content.push_str("<small>JMdict and JMnedict by EDRDG, CC BY-SA 3.0.</small><ul>");
    for (part_of_speech, english, extra) in definitions {
        if part_of_speech == "Wikipedia definition" {
            content.push_str("</ul><small>Wikipedia, CC BY-SA 3.0.</small><ul>");
        }
        let part = if part_of_speech.is_empty() {
            String::new()
        } else {
            format!("<i>{part_of_speech}</i>: ")
        };
        let note = if extra.is_empty() {
            String::new()
        } else {
            format!(" ({extra})")
        };
        content.push_str(&format!("<li>{part}{english}{note}</li>"));
    }
    content.push_str("</ul>");
    Infobox {
        id: format!("jisho-{}", url.rsplit('/').next().unwrap_or_default()),
        title: forms.first().cloned().unwrap_or_default(),
        content,
        url: Some(url.to_string()),
        urls: vec![InfoboxUrl {
            title: "Jisho.org".to_string(),
            url: url.to_string(),
        }],
        ..Default::default()
    }
}

impl Engine for Jisho {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let url = format!(
            "{}/api/v1/search/words?keyword={}",
            site(&self.meta),
            encode(&query.query_text())
        );
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(pages) = json_path(&json, "data").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let base = format!("{}/word/", site(&self.meta));
        let mut items = Vec::new();
        let mut first = true;
        for page in pages {
            let Some(slug) = page.get("slug").and_then(Value::as_str) else {
                continue;
            };
            let Some(url) = absolute_url(slug, &base) else {
                continue;
            };
            if slug.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let forms = alternative_forms(page);
            if forms.is_empty() {
                continue;
            }
            let definitions = sense_definitions(page);
            let sentence = definitions
                .iter()
                .map(|(_, english, _)| format!("{english}."))
                .collect::<Vec<_>>()
                .join(" ");
            let mut chars = sentence.chars();
            let head: String = chars.by_ref().take(300).collect();
            let content = if chars.next().is_some() {
                format!("{head}...")
            } else {
                head
            };
            let title = forms.join(", ");
            items.push(
                ResultBuilder::new(url.clone(), title)
                    .content(content)
                    .build(),
            );
            if first {
                first = false;
                items.push(EngineItem::Infobox(infobox(&forms, &url, &definitions)));
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Jisho> {
        Jisho::from_config(&EngineConfig::new(
            "jisho",
            "jisho",
            "js",
            &["dictionaries"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    fn body(value: &Value) -> EngineResponse {
        EngineResponse::new(200, SITE, value.to_string().into_bytes())
    }

    #[test]
    fn builds_the_api_url() {
        assert_eq!(
            engine().build_request(&query("neko"), 1).unwrap().url,
            "https://jisho.org/api/v1/search/words?keyword=neko"
        );
    }

    #[test]
    fn parses_entries_and_the_first_infobox() {
        let document = serde_json::json!({
            "data": [{
                "slug": "neko",
                "japanese": [{"word": "猫", "reading": "ねこ"}, {"reading": "ネコ"}],
                "senses": [{"parts_of_speech": ["Noun"], "english_definitions": ["cat", "feline"],
                            "tags": ["usually written using kana alone"], "info": ["kana"]}]
            }]
        });
        let items = engine().parse(&body(&document), &query("neko")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "猫 (ねこ), ネコ");
                assert_eq!(result.url, "https://jisho.org/word/neko");
                assert_eq!(result.content, "cat; feline.");
            }
            other => panic!("expected a result, got {other:?}"),
        }
        match &items[1] {
            EngineItem::Infobox(infobox) => {
                assert_eq!(infobox.title, "猫 (ねこ)");
                assert_eq!(infobox.urls[0].url, "https://jisho.org/word/neko");
            }
            other => panic!("expected an infobox, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_answer_is_empty() {
        assert!(
            engine()
                .parse(&body(&serde_json::json!({})), &query("neko"))
                .unwrap()
                .is_empty()
        );
    }
}
