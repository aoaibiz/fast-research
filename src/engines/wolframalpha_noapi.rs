//! Wolfram|Alpha no-API engine, ported from searx/engines/wolframalpha_noapi.py.
// engine-id: wolframalpha_noapi

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
    shortcut: "wa",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

const SEARCH_URL: &str = "https://www.wolframalpha.com/input/json.jsp?async=false&banners=raw&debuggingdata=false&format=image,plaintext,imagemap,minput,moutput&formattimeout=2&input={INPUT}&output=JSON&parsetimeout=2&proxycode={TOKEN}&scantimeout=0.5&sponsorcategories=true&statemethod=deploybutton";
const SITE_URL: &str = "https://www.wolframalpha.com/input/?i=";
const IMAGE_PODS: [&str; 3] = ["VisualRepresentation", "Illustration", "Symbol"];

/// Wolfram|Alpha through the website JSON endpoint (no API key).
pub struct WolframAlphaNoapi {
    meta: EngineMeta,
}

impl WolframAlphaNoapi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn referer_url(text: &str) -> String {
    format!("{SITE_URL}{}", encode(text))
}

impl Engine for WolframAlphaNoapi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // Upstream obtains the proxycode from /input/api/v1/code before the
        // query; the frozen interface cannot do that round trip, so the token
        // is read from the engine settings.
        let text = query.query_text();
        let token = self.meta.opt_str("token").unwrap_or_default();
        let url = SEARCH_URL
            .replace("{INPUT}", &encode(&text))
            .replace("{TOKEN}", token);
        Ok(EngineRequest::get(url).header("Referer", &referer_url(&text)))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        if !json_path(&json, "queryresult.success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Ok(Vec::new());
        }
        let pods = match json_path(&json, "queryresult.pods").and_then(|value| value.as_array()) {
            Some(pods) => pods,
            None => return Ok(Vec::new()),
        };
        let mut title = String::new();
        let mut chunks: Vec<(String, String)> = Vec::new();
        let mut images: Vec<String> = Vec::new();
        let mut content = String::new();
        for pod in pods {
            let pod_id = json_str(pod, "id");
            let pod_title = json_str(pod, "title");
            let pod_is_result = json_path(pod, "primary")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let subpods = match json_path(pod, "subpods").and_then(|value| value.as_array()) {
                Some(subpods) => subpods,
                None => continue,
            };
            if let Some(first) = subpods.first() {
                if pod_id == "Input" || title.is_empty() {
                    title = json_str(first, "plaintext");
                }
            }
            for subpod in subpods {
                let plaintext = json_str(subpod, "plaintext");
                if !plaintext.is_empty() && !IMAGE_PODS.contains(&pod_id.as_str()) {
                    if plaintext != "(requires interactivity)" {
                        chunks.push((pod_title.clone(), plaintext.clone()));
                    }
                    if (pod_is_result || content.is_empty()) && pod_id != "Input" {
                        content = format!("{pod_title}: {plaintext}");
                    }
                } else if let Some(src) = json_path(subpod, "img").and_then(|value| value.as_str())
                {
                    images.push(src.to_string());
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
            ResultBuilder::new(referer, format!("Wolfram|Alpha ({title})"))
                .content(content)
                .build(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"queryresult":{"success":true,"pods":[{"id":"Input","title":"Input","subpods":[{"plaintext":"2+2"}]},{"id":"Result","title":"Result","primary":true,"subpods":[{"plaintext":"4"}]},{"id":"VisualRepresentation","title":"Plot","subpods":[{"img":"https://example.com/v.png"}]}]}}"##;

    fn engine() -> Arc<WolframAlphaNoapi> {
        let mut config =
            EngineConfig::new("wolframalpha", "wolframalpha_noapi", "wa", &["general"]);
        config
            .extra
            .insert("token".into(), toml::Value::String("TOKEN".into()));
        WolframAlphaNoapi::from_config(&config).unwrap()
    }

    fn query() -> Query {
        Query::parse("2+2", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_pods_into_an_infobox() {
        let response = EngineResponse::new(
            200,
            "https://www.wolframalpha.com/",
            FIXTURE.as_bytes().to_vec(),
        );
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Infobox(infobox) = &items[0] else {
            panic!("expected an infobox")
        };
        assert_eq!(infobox.title, "2+2");
        assert_eq!(
            infobox.attributes.get("Result").map(String::as_str),
            Some("4")
        );
        assert_eq!(
            infobox.img_src.as_deref(),
            Some("https://example.com/v.png")
        );
        let referer = "https://www.wolframalpha.com/input/?i=2%2B2";
        assert_eq!(infobox.urls[0].url, referer);
        let EngineItem::Result(result) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, referer);
        assert_eq!(result.title, "Wolfram|Alpha (2+2)");
        assert_eq!(result.content, "Result: 4");
    }

    #[test]
    fn unsuccessful_queries_are_empty() {
        let body = r##"{"queryresult":{"success":false,"pods":[]}}"##;
        let response = EngineResponse::new(
            200,
            "https://www.wolframalpha.com/",
            body.as_bytes().to_vec(),
        );
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }

    #[test]
    fn builds_the_json_request() {
        let request = engine().build_request(&query(), 1).unwrap();
        for part in [
            "input/json.jsp",
            "input=2%2B2",
            "proxycode=TOKEN",
            "output=JSON",
        ] {
            assert!(
                request.url.contains(part),
                "missing {part} in {}",
                request.url
            );
        }
        let referer = request
            .headers
            .get("referer")
            .and_then(|value| value.to_str().ok());
        assert_eq!(referer, Some("https://www.wolframalpha.com/input/?i=2%2B2"));
    }
}
