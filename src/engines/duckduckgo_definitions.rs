//! DuckDuckGo Instant Answer engine, ported from searx/engines/duckduckgo_definitions.py.
//! The external URL, area and coordinate tables of the upstream infobox are not ported.
// engine-id: duckduckgo_definitions

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::{Infobox, InfoboxUrl};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "ddd",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.duckduckgo.com/";

pub struct DuckDuckGoDefinitions {
    meta: EngineMeta,
}

impl DuckDuckGoDefinitions {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for DuckDuckGoDefinitions {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("q", query.query_text()),
            ("format", "json".to_string()),
            ("pretty", "0".to_string()),
            ("no_redirect", "1".to_string()),
            ("d", "1".to_string()),
        ];
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let url = format!("{}/?{}", base.trim_end_matches('/'), query_string(&pairs));
        Ok(EngineRequest::get(url).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let mut items = Vec::new();
        let heading = json_str(&json, "Heading");
        let answer = json_str(&json, "Answer");
        if !answer.is_empty() && !matches!(json_str(&json, "AnswerType").as_str(), "calc" | "ip") {
            items.push(EngineItem::Answer(strip_tags(&answer)));
        }
        let mut content = strip_tags(&json_str(&json, "Definition"));
        content.push_str(&strip_tags(&json_str(&json, "Abstract")));
        let image = absolute_url(&json_str(&json, "Image"), "https://duckduckgo.com/")
            .filter(|url| url.starts_with("http"));
        let mut urls: Vec<InfoboxUrl> = Vec::new();
        for entry in array(&json, "Results") {
            let first_url = json_str(entry, "FirstURL");
            let text = json_str(entry, "Text");
            if !first_url.starts_with("http") || text.is_empty() {
                continue;
            }
            if !heading.is_empty() {
                items.push(result(first_url.clone(), &heading, ""));
            }
            urls.push(InfoboxUrl {
                title: text,
                url: first_url,
            });
        }
        let mut groups = 0usize;
        for entry in array(&json, "RelatedTopics") {
            if entry.get("FirstURL").is_some() {
                items.extend(suggestion(&heading, entry));
            } else if let Some(topics) = entry.get("Topics").and_then(|value| value.as_array()) {
                groups += 1;
                for topic in topics {
                    items.extend(suggestion(&heading, topic));
                }
            }
        }
        let mut infobox_id = String::new();
        for (url_key, source_key, as_result) in [
            ("AbstractURL", "AbstractSource", true),
            ("DefinitionURL", "DefinitionSource", false),
        ] {
            let link = json_str(&json, url_key);
            if link.is_empty() {
                continue;
            }
            urls.push(InfoboxUrl {
                title: json_str(&json, source_key),
                url: link.clone(),
            });
            if as_result && !heading.is_empty() && link.starts_with("http") {
                items.push(result(link.clone(), &heading, ""));
            }
            infobox_id = link;
        }
        if !infobox_id.is_empty() {
            infobox_id = infobox_id.replace("http:", "https:");
        }
        let mut attributes: BTreeMap<String, String> = BTreeMap::new();
        for info in array(&json, "Infobox.content") {
            let label = json_str(info, "label");
            let value = json_str(info, "value");
            if json_str(info, "data_type") == "string" && label != "Website" && !value.is_empty() {
                attributes.insert(label, value);
            }
        }
        if !heading.is_empty() {
            let plain = image.is_none()
                && attributes.is_empty()
                && urls.len() == 1
                && groups == 0
                && content.is_empty();
            if plain && !urls.is_empty() {
                items.push(result(urls[0].url.clone(), &heading, ""));
            } else {
                items.push(EngineItem::Infobox(Infobox {
                    id: infobox_id,
                    title: heading,
                    content,
                    img_src: image,
                    urls,
                    attributes,
                    ..Default::default()
                }));
            }
        }
        Ok(items)
    }
}

/// The array at a JSON path, empty when the container is missing.
fn array<'a>(json: &'a serde_json::Value, path: &str) -> &'a [serde_json::Value] {
    let values = json_path(json, path).and_then(|value| value.as_array());
    values.map(Vec::as_slice).unwrap_or_default()
}

/// The first anchor of a related topic, upstream's result_to_text.
fn result_to_text(raw: &str, snippet: &str) -> Option<String> {
    let document = parse_html(snippet);
    let anchor = select_elements(&document, "a")
        .ok()
        .and_then(|links| links.first().copied());
    let value = match anchor {
        Some(anchor) => text(anchor),
        None => clean(raw),
    };
    let broken = value.starts_with("http") && value.contains(' ');
    (!value.is_empty() && !broken).then_some(value)
}

/// One related topic entry as a suggestion, unless it is broken or the heading.
fn suggestion(heading: &str, entry: &serde_json::Value) -> Option<EngineItem> {
    let text = json_str(entry, "Text");
    if text.starts_with("http") && text.contains(' ') {
        return None;
    }
    let value = result_to_text(&text, &json_str(entry, "Result"))?;
    (value != heading).then_some(EngineItem::Suggestion(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<DuckDuckGoDefinitions> {
        let config = EngineConfig::new(
            "ddg definitions",
            "duckduckgo_definitions",
            "ddd",
            &["general"],
        );
        DuckDuckGoDefinitions::from_config(&config).unwrap()
    }

    const FIXTURE: &str = r##"{"Heading":"Rust","Abstract":"Rust is a language.","AbstractURL":"https://en.wikipedia.org/wiki/Rust","AbstractSource":"Wikipedia","Image":"/i/rust.png","Results":[{"FirstURL":"https://www.rust-lang.org/","Text":"Rust website"}],"RelatedTopics":[{"FirstURL":"https://example.com/a","Text":"Rust arena","Result":"<a href='https://example.com/a'>Rust arena</a>"},{"Name":"Topics","Topics":[{"FirstURL":"https://example.com/b","Text":"More Rust"}]}],"Infobox":{"content":[{"data_type":"string","label":"Designed by","value":"Graydon Hoare"},{"data_type":"string","label":"Website","value":"rust-lang.org"}]}}"##;

    #[test]
    fn parses_the_instant_answer() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert!(
            matches!(&items[0], EngineItem::Result(item) if item.url == "https://www.rust-lang.org/")
        );
        assert!(matches!(&items[1], EngineItem::Suggestion(item) if item == "Rust arena"));
        assert!(matches!(&items[2], EngineItem::Suggestion(item) if item == "More Rust"));
        let EngineItem::Infobox(infobox) = &items[4] else {
            panic!("expected an infobox")
        };
        assert_eq!(infobox.title, "Rust");
        assert_eq!(infobox.content, "Rust is a language.");
        assert_eq!(infobox.id, "https://en.wikipedia.org/wiki/Rust");
        assert_eq!(
            infobox.img_src.as_deref(),
            Some("https://duckduckgo.com/i/rust.png")
        );
        assert_eq!(infobox.urls.len(), 2);
        assert_eq!(
            infobox.attributes.get("Designed by").map(String::as_str),
            Some("Graydon Hoare")
        );
        assert!(!infobox.attributes.contains_key("Website"));
    }

    #[test]
    fn builds_the_instant_answer_url() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.starts_with("https://api.duckduckgo.com/?"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("format=json") && request.url.contains("no_redirect=1"));
    }
}
