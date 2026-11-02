//! Wolfram|Alpha API engine, ported from searx/engines/wolframalpha_api.py.
// engine-id: wolframalpha_api

use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::{Infobox, InfoboxUrl};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "waa",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const API_URL: &str = "https://api.wolframalpha.com/v2/query";
const SITE_URL: &str = "https://www.wolframalpha.com/input/?i=";
const IMAGE_PODS: [&str; 2] = ["VisualRepresentation", "Illustration"];

/// Wolfram|Alpha through the official API. Requires an appid.
pub struct WolframAlphaApi {
    meta: EngineMeta,
}

impl WolframAlphaApi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn referer_url(text: &str) -> String {
    format!("{SITE_URL}{}", encode(text))
}

impl Engine for WolframAlphaApi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.meta.api_key.as_deref().unwrap_or_default();
        let text = query.query_text();
        Ok(
            EngineRequest::get(format!("{API_URL}?appid={key}&input={}", encode(&text)))
                .header("Referer", &referer_url(&text)),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let parsed = parse_query_result(&response.text())?;
        if !parsed.success {
            return Ok(Vec::new());
        }
        let mut title = parsed
            .pods
            .iter()
            .find(|pod| pod.id.starts_with("Input"))
            .and_then(|pod| pod.subpods.first())
            .map(|subpod| subpod.plaintext.clone())
            .unwrap_or_default();
        let mut chunks: Vec<(String, String)> = Vec::new();
        let mut images: Vec<String> = Vec::new();
        let mut content = String::new();
        for pod in &parsed.pods {
            for subpod in &pod.subpods {
                if !subpod.plaintext.is_empty() && !IMAGE_PODS.contains(&pod.id.as_str()) {
                    if (pod.primary || content.is_empty()) && pod.id != "Input" {
                        content = format!("{}: {}", pod.title, subpod.plaintext);
                    }
                    if title.is_empty() {
                        title = subpod.plaintext.clone();
                    }
                    chunks.push((pod.title.clone(), replace_pua_chars(&subpod.plaintext)));
                } else if !subpod.src.is_empty() {
                    images.push(subpod.src.clone());
                }
            }
        }
        if chunks.is_empty() && images.is_empty() {
            return Ok(Vec::new());
        }
        let referer = referer_url(&query.query_text());
        let infobox = Infobox {
            title: title.clone(),
            attributes: chunks.into_iter().collect(),
            urls: vec![InfoboxUrl {
                title: "Wolfram|Alpha".to_string(),
                url: referer.clone(),
            }],
            img_src: images.first().cloned(),
            ..Default::default()
        };
        Ok(vec![
            EngineItem::Infobox(infobox),
            ResultBuilder::new(referer, format!("Wolfram Alpha ({title})"))
                .content(content)
                .build(),
        ])
    }
}

#[derive(Default)]
struct Subpod {
    plaintext: String,
    src: String,
}

#[derive(Default)]
struct Pod {
    id: String,
    title: String,
    primary: bool,
    subpods: Vec<Subpod>,
}

struct QueryResult {
    success: bool,
    pods: Vec<Pod>,
}

fn attribute<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, value)| value.as_str())
}

fn attributes_of(start: &BytesStart<'_>) -> Vec<(String, String)> {
    start
        .attributes()
        .flatten()
        .map(|a| {
            (
                String::from_utf8_lossy(a.key.local_name().as_ref()).to_string(),
                a.normalized_value(quick_xml::XmlVersion::Explicit1_0)
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

fn apply_image(subpod: &mut Option<Subpod>, start: &BytesStart<'_>) {
    let src = attribute(&attributes_of(start), "src").map(|src| src.to_string());
    if let (Some(subpod), Some(src)) = (subpod.as_mut(), src) {
        subpod.src = src;
    }
}

/// Read the queryresult document of the API (upstream uses lxml xpath).
fn parse_query_result(body: &str) -> Result<QueryResult, EngineError> {
    let mut reader = Reader::from_str(body);
    let mut buffer = Vec::new();
    let mut result = QueryResult {
        success: true,
        pods: Vec::new(),
    };
    let mut pod: Option<Pod> = None;
    let mut subpod: Option<Subpod> = None;
    let mut in_plaintext = false;
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            EngineError::Parse(format!("invalid WolframAlpha response: {error}"))
        })?;
        match event {
            Event::Start(start) => match start.local_name().as_ref() {
                b"queryresult" => {
                    result.success = attribute(&attributes_of(&start), "success") != Some("false")
                }
                b"pod" => {
                    let attributes = attributes_of(&start);
                    pod = Some(Pod {
                        id: attribute(&attributes, "id").unwrap_or_default().to_string(),
                        title: attribute(&attributes, "title")
                            .unwrap_or_default()
                            .to_string(),
                        primary: attribute(&attributes, "primary").is_some(),
                        subpods: Vec::new(),
                    });
                }
                b"subpod" => subpod = Some(Subpod::default()),
                b"plaintext" => in_plaintext = true,
                b"img" => apply_image(&mut subpod, &start),
                _ => {}
            },
            Event::Empty(start) => {
                if start.local_name().as_ref() == b"img" {
                    apply_image(&mut subpod, &start);
                }
            }
            Event::Text(text) => {
                if in_plaintext {
                    if let (Ok(value), Some(subpod)) = (
                        text.xml_content(quick_xml::XmlVersion::Explicit1_0),
                        subpod.as_mut(),
                    ) {
                        subpod.plaintext.push_str(&value);
                    }
                }
            }
            Event::End(end) => match end.local_name().as_ref() {
                b"plaintext" => in_plaintext = false,
                b"subpod" => {
                    if let (Some(pod), Some(open)) = (pod.as_mut(), subpod.take()) {
                        pod.subpods.push(open);
                    }
                }
                b"pod" => {
                    if let Some(open) = pod.take() {
                        result.pods.push(open);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if let Some(open) = pod.take() {
        result.pods.push(open);
    }
    Ok(result)
}

/// Upstream replaces the private use area glyphs of the API with readable text.
fn replace_pua_chars(text: &str) -> String {
    text.replace('\u{f522}', "\u{2192}")
        .replace('\u{f7b1}', "\u{2115}")
        .replace('\u{f7b4}', "\u{211a}")
        .replace('\u{f7b5}', "\u{211d}")
        .replace('\u{f7bd}', "\u{2124}")
        .replace('\u{f74c}', "d")
        .replace('\u{f74d}', "\u{212f}")
        .replace('\u{f74e}', "i")
        .replace('\u{f7d9}', "=")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<queryresult success="true"><pod title="Input" id="Input"><subpod><plaintext>integrate x</plaintext></subpod></pod><pod title="Result" id="Result" primary="true"><subpod><plaintext>x = 1 + 2</plaintext></subpod></pod><pod title="Plot" id="VisualRepresentation"><subpod><img src="https://example.com/plot.png" alt="plot" /></subpod></pod></queryresult>"##;

    fn engine() -> Arc<WolframAlphaApi> {
        let mut config =
            EngineConfig::new("wolframalpha_api", "wolframalpha_api", "waa", &["general"]);
        config.api_key = Some("KEY".to_string());
        WolframAlphaApi::from_config(&config).unwrap()
    }
    fn query() -> Query {
        Query::parse(
            "integrate x",
            &crate::engine::EngineRegistry::default(),
            &[],
        )
    }

    #[test]
    fn parses_pods_into_an_infobox() {
        let response = EngineResponse::new(200, API_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Infobox(infobox) = &items[0] else {
            panic!("expected an infobox")
        };
        assert_eq!(infobox.title, "integrate x");
        assert_eq!(
            infobox.attributes.get("Result").map(String::as_str),
            Some("x = 1 + 2")
        );
        assert_eq!(
            infobox.img_src.as_deref(),
            Some("https://example.com/plot.png")
        );
        let referer = "https://www.wolframalpha.com/input/?i=integrate%20x";
        assert_eq!(infobox.urls[0].url, referer);
        let EngineItem::Result(result) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, referer);
        assert_eq!(result.title, "Wolfram Alpha (integrate x)");
        assert_eq!(result.content, "Result: x = 1 + 2");
        let failed = r##"<queryresult success="false"></queryresult>"##;
        let response = EngineResponse::new(200, API_URL, failed.as_bytes().to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }

    #[test]
    fn builds_the_api_request() {
        let request = engine().build_request(&query(), 1).unwrap();
        assert_eq!(
            request.url,
            "https://api.wolframalpha.com/v2/query?appid=KEY&input=integrate%20x"
        );
        let referer = request
            .headers
            .get("referer")
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            referer,
            Some("https://www.wolframalpha.com/input/?i=integrate%20x")
        );
    }
}
