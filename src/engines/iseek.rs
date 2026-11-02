//! iSeek engine, ported from searx/engines/iseek.py.
//!
//! The qToken is deterministic: upstream derives it from the query hash so
//! that paging does not need a second request to obtain a token.
// engine-id: iseek

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "isk",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://api.iseek.com";
const PAGE_SIZE: u32 = 10;
/// The API stops serving documents after the second page.
const MAX_OFFSET: u32 = 20;

/// iSeek, a general web search by Vantage Labs.
pub struct Iseek {
    meta: EngineMeta,
}

impl Iseek {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Iseek {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pageno = pageno.max(1);
        let offset = (pageno - 1).saturating_mul(PAGE_SIZE);
        if offset >= MAX_OFFSET {
            return Err(EngineError::Message(
                "iseek serves at most 20 results (two pages)".into(),
            ));
        }
        let text = query.query_text();
        let pairs = vec![
            ("q", text.clone()),
            ("key", "core-web".to_string()),
            ("num", PAGE_SIZE.to_string()),
            ("off", offset.to_string()),
            ("rSort", "__metasearch_score_d:desc".to_string()),
            ("names", "title_t,content_txt,url_s".to_string()),
            ("qNames", "title_t".to_string()),
            ("qToken", token(&text, pageno)),
        ];
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/');
        Ok(
            EngineRequest::get(format!("{base}/search?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(groups) = json_path(&json, "data").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for group in groups {
            let Some(documents) =
                json_path(group, "doclist.docs").and_then(|value| value.as_array())
            else {
                continue;
            };
            for document in documents {
                let url = json_str(document, "url_s");
                let title = json_str(document, "title_t");
                if url.is_empty() || title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(url, title)
                        .content(joined_content(document))
                        .build(),
                );
            }
        }
        Ok(results)
    }
}

/// Upstream joins the list of highlighted text fragments.
fn joined_content(document: &serde_json::Value) -> String {
    match json_path(document, "content_txt").and_then(|value| value.as_array()) {
        Some(parts) => parts
            .iter()
            .filter_map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join(""),
        None => String::new(),
    }
}

/// The qToken: first four base64 characters of the SHA-256 of the query,
/// suffixed with the page number.
fn token(query: &str, pageno: u32) -> String {
    let digest = Sha256::digest(query.as_bytes());
    format!(
        "{}_{}",
        base64_prefix([digest[0], digest[1], digest[2]]),
        pageno
    )
}

fn base64_prefix(bytes: [u8; 3]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
    let mut output = String::with_capacity(4);
    for shift in [18, 12, 6, 0] {
        output.push(ALPHABET[((value >> shift) & 63) as usize] as char);
    }
    output
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Iseek> {
        Iseek::from_config(&EngineConfig::new("iseek", "iseek", "isk", &["general"]))
            .expect("iseek engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn the_token_is_derived_from_the_query_hash() {
        assert_eq!(token("rust", 1), "Uh/l_1");
        assert_eq!(token("rust", 2), "Uh/l_2");
        assert_eq!(token("hello world", 1), "uU0n_1");
    }

    #[test]
    fn builds_the_paged_api_url() {
        let request = engine().build_request(&query("rust"), 2).unwrap();
        assert!(request.url.starts_with("https://api.iseek.com/search?"));
        assert!(request.url.contains("q=rust"));
        assert!(request.url.contains("off=10"));
        assert!(request.url.contains("qToken=Uh%2Fl_2"));
        assert!(request.url.contains("rSort=__metasearch_score_d%3Adesc"));
    }

    #[test]
    fn only_the_first_two_pages_are_available() {
        assert!(engine().build_request(&query("rust"), 3).is_err());
    }

    #[test]
    fn parses_the_grouped_documents() {
        let body = r##"{"data":[{"doclist":{"docs":[{"url_s":"https://example.com/a","title_t":"A","content_txt":["Hello ","world"]},{"url_s":"javascript:alert(1)","title_t":"Bad","content_txt":[]}]}}]}"##;
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/a");
                assert_eq!(result.title, "A");
                assert_eq!(result.content, "Hello world");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn a_response_without_data_is_empty() {
        let response = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        assert!(
            engine()
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
