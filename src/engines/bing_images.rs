//! Bing-Images engine, ported from `searx/engines/bing_images.py`.

// engine-id: bing_images

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "web"],
    shortcut: "bii",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.bing.com/images/async";

fn time_map(range: TimeRange) -> i64 {
    match range {
        TimeRange::Day => 60 * 24,
        TimeRange::Week => 60 * 24 * 7,
        TimeRange::Month => 60 * 24 * 31,
        TimeRange::Year => 60 * 24 * 365,
    }
}

/// Bing image search.
pub struct BingImages {
    meta: EngineMeta,
}

impl BingImages {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for BingImages {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("mmasync", "1".to_string()),
            ("first", ((pageno.saturating_sub(1)) * 35 + 1).to_string()),
            ("count", "35".to_string()),
        ];
        let language = locale_language(&query.locale);
        let region = locale_region(&query.locale);
        if language != "all" && region != "all" {
            pairs.push(("setlang", language.clone()));
            pairs.push(("cc", region));
        }
        if let Some(range) = query.time_range {
            pairs.push(("qft", format!("filterui:age-lt{}", time_map(range))));
        }
        Ok(
            EngineRequest::get(format!("{BASE_URL}?{}", query_string(&pairs)))
                .header("Accept-Language", &format!("{language};q=0.9")),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for element in select_elements(&html, "ul[class*='dgControl_list'] li")? {
            // The metadata of a result sits in the "m" attribute of the iusc anchor.
            let Some(metadata) = attr_of(element, "a.iusc", "m").map(|value| clean(&value)) else {
                continue;
            };
            if metadata.is_empty() {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&metadata) else {
                continue;
            };
            let url = json_str(&json, "purl");
            let img_src = json_str(&json, "murl");
            let thumbnail = json_str(&json, "turl");
            let description = json_str(&json, "desc");
            if url.is_empty() || img_src.is_empty() {
                continue;
            }
            let title = text_of(element, "div.infnmpt")
                .or_else(|| text_of(element, "div.infnmpt a"))
                .unwrap_or_default();
            let source = text_of(element, "div.imgpt div.lnkw a").unwrap_or_default();
            let format_text = text_of(element, "div.imgpt div span")
                .unwrap_or_default()
                .replace(" · ", "|");
            let mut parts = format_text.split('|').map(str::trim);
            let resolution = parts.next().unwrap_or("").to_string();
            let img_format = parts.next().unwrap_or("").to_string();
            let mut builder = ResultBuilder::new(url, title)
                .content(description)
                .image(img_src)
                .thumbnail(thumbnail)
                .template(Template::Images);
            if !source.is_empty() {
                builder = builder.metadata("source", source);
            }
            if !resolution.is_empty() {
                builder = builder.metadata("resolution", resolution);
            }
            if !img_format.is_empty() {
                builder = builder.metadata("img_format", img_format);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_results_json_metadata() {
        let body = r#"<div id="b_content">
          <ul class="dgControl_list">
            <li>
              <a class="iusc" m='{"purl":"https://example.com/full.jpg","murl":"https://img.example.com/x.jpg","turl":"https://img.example.com/t.jpg","desc":"An example image."}'>
                <div class="infnmpt">Example</div>
                <div class="imgpt"><div><span>1024 x 768 · JPEG</span></div><div class="lnkw"><a>example.com</a></div></div>
              </a>
            </li>
          </ul>
        </div>"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine = BingImages::from_config(&EngineConfig::new(
            "bing images",
            "bing_images",
            "bii",
            &["images"],
        ))
        .unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/full.jpg");
                assert_eq!(result.title, "Example");
                assert_eq!(
                    result.img_src.as_deref(),
                    Some("https://img.example.com/x.jpg")
                );
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn time_range_is_forwarded() {
        let engine = BingImages::from_config(&EngineConfig::new(
            "bing images",
            "bing_images",
            "bii",
            &["images"],
        ))
        .unwrap();
        let mut query = Query::parse("cat", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .contains(&format!("qft=filterui%3Aage-lt{}", 60 * 24 * 7))
        );
        assert!(request.url.contains("first=36"));
    }
}
