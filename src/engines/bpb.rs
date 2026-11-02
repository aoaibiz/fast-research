//! BPB engine, ported from `searx/engines/bpb.py`.

// engine-id: bpb

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "bpb",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.bpb.de";

/// Bundeszentrale für politische Bildung.
pub struct Bpb {
    meta: EngineMeta,
}

impl Bpb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Bpb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("query[term]", query.query_text()),
            ("page", pageno.saturating_sub(1).to_string()),
            ("sort[direction]", "descending".to_string()),
            ("payload[nid]", "65350".to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/bpbapi/filter/search?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let items = match json_path(&json, "teaser").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "teaser.title");
            let url = json_str(item, "teaser.link.url");
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let url = format!("{BASE_URL}{url}");
            let thumbnail = match json_path(item, "teaser.image.sources")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.last())
            {
                Some(source) => json_str(source, "url"),
                None => String::new(),
            };
            let mut metadata: Vec<String> = Vec::new();
            let overline = json_str(item, "extension.overline");
            if !overline.is_empty() {
                metadata.push(overline);
            }
            let authors: Vec<String> = json_path(item, "extension.authors")
                .and_then(|v| v.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !authors.is_empty() {
                metadata.push(authors.join(", "));
            }
            let mut builder = ResultBuilder::new(url, title)
                .content(json_str(item, "teaser.text"))
                .published_date(json_str(item, "extension.publishingDate"));
            if !thumbnail.is_empty() {
                builder = builder.thumbnail(format!("{BASE_URL}{thumbnail}"));
            }
            if !metadata.is_empty() {
                builder = builder.metadata("metadata", metadata.join(" | "));
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
    fn parses_teasers() {
        let body = r#"{"teaser":[
          {"teaser":{"title":"Politics 101","link":{"url":"/artikel/123/"},
                     "text":"About politics.",
                     "image":{"sources":[{"url":"/t-small.jpg"},{"url":"/t-big.jpg"}]}},
           "extension":{"overline":"Basiswissen","publishingDate":"1700000000",
                        "authors":[{"name":"Jane"}]}}
        ]}"#;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let engine =
            Bpb::from_config(&EngineConfig::new("bpb", "bpb", "bpb", &["general"])).unwrap();
        let items = engine
            .parse(
                &response,
                &Query::parse("politik", &crate::engine::EngineRegistry::default(), &[]),
            )
            .unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.bpb.de/artikel/123/");
                assert_eq!(result.title, "Politics 101");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://www.bpb.de/t-big.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }
}
