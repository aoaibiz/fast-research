//! Il Post engine, ported from searx/engines/il_post.py.
//!
//! Only the month and year time ranges are supported; upstream returns no
//! request for the other ranges, which is reported as an engine error here.
// engine-id: il_post

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "pst",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://api.ilpost.org/search/api/site_search/";

/// The Italian newspaper Il Post.
pub struct IlPost {
    meta: EngineMeta,
}

impl IlPost {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for IlPost {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let mut filters = "ctype:articoli".to_string();
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Month => "pub_date:ultimi_30_giorni",
                TimeRange::Year => "pub_date:ultimo_anno",
                _ => {
                    return Err(EngineError::Message(
                        "il_post supports only the month and year time ranges".into(),
                    ));
                }
            };
            filters.push(';');
            filters.push_str(value);
        }
        let pairs = vec![
            ("qs", query.query_text()),
            ("pg", pageno.to_string()),
            ("sort", "date_d".to_string()),
            ("filters", filters),
        ];
        Ok(EngineRequest::get(format!(
            "{}?{}",
            SEARCH_API.trim_end_matches('/'),
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
        let Some(items) = json_path(&json, "docs").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "link");
            let title = json_str(item, "title");
            if url.is_empty() || title.is_empty() || !is_http_url(&url) {
                continue;
            }
            let mut builder =
                ResultBuilder::new(url, title).content(strip_tags(&json_str(item, "summary")));
            let image = json_str(item, "image");
            if is_http_url(&image) {
                builder = builder.thumbnail(image);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<IlPost> {
        IlPost::from_config(&EngineConfig::new("il post", "il_post", "pst", &["news"])).unwrap()
    }

    #[test]
    fn parses_the_docs_list() {
        let body = r##"{"docs":[{"link":"https://www.ilpost.it/2024/01/01/articolo/","title":"Un articolo","summary":"Il sommario","image":"https://www.ilpost.it/img.jpg"},{"link":"","title":"senza link"},{"link":"ftp://example.com/x","title":"non http"}]}"##;
        let response = EngineResponse::new(200, SEARCH_API, body.as_bytes().to_vec());
        let query = Query::parse("elezioni", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.ilpost.it/2024/01/01/articolo/");
                assert_eq!(result.title, "Un articolo");
                assert_eq!(result.content, "Il sommario");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://www.ilpost.it/img.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_time_range_filter() {
        let mut query = Query::parse("elezioni", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Year);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with(SEARCH_API.trim_end_matches('/')));
        assert!(request.url.contains("qs=elezioni"));
        assert!(request.url.contains("pg=2"));
        assert!(
            request
                .url
                .contains("filters=ctype%3Aarticoli%3Bpub_date%3Aultimo_anno")
        );
    }

    #[test]
    fn unsupported_time_ranges_are_rejected() {
        let mut query = Query::parse("elezioni", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Day);
        assert!(engine().build_request(&query, 1).is_err());
    }
}
