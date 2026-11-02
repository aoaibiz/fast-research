//! Elasticsearch engine, ported from searx/engines/elasticsearch.py.
//!
//! Elasticsearch is a self-hosted Lucene backend; the engine queries one index
//! of an instance configured with base_url and index.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: elasticsearch

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "els",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// An Elasticsearch index.
pub struct Elasticsearch {
    meta: EngineMeta,
    index: String,
    query_type: String,
    custom_query_json: String,
    username: String,
    password: String,
}

impl Elasticsearch {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let index = meta.opt_str("index").unwrap_or_default().to_string();
        if index.is_empty() {
            return Err("elasticsearch requires the index option".into());
        }
        let query_type = meta.opt_str("query_type").unwrap_or("match").to_string();
        let custom_query_json = meta
            .opt_str("custom_query_json")
            .unwrap_or_default()
            .to_string();
        if query_type == "custom" && custom_query_json.is_empty() {
            return Err("elasticsearch custom query_type needs custom_query_json".into());
        }
        let username = meta.opt_str("username").unwrap_or_default().to_string();
        let password = meta.opt_str("password").unwrap_or_default().to_string();
        Ok(Arc::new(Self {
            meta,
            index,
            query_type,
            custom_query_json,
            username,
            password,
        }))
    }

    /// The JSON request body for the configured query type (upstream payload).
    fn payload(&self, query: &Query) -> serde_json::Value {
        let text = query.query_text();
        if self.query_type == "custom" {
            return serde_json::from_str(&self.custom_query_json)
                .unwrap_or_else(|_| serde_json::json!({ "query": { "match_all": {} } }));
        }
        let clause = match self.query_type.as_str() {
            "simple_query_string" => serde_json::json!({
                "simple_query_string": { "query": text }
            }),
            "term" => serde_json::json!({ "term": { "_all": text } }),
            "terms" => serde_json::json!({ "terms": { "_all": [text] } }),
            _ => serde_json::json!({ "match": { "_all": text } }),
        };
        serde_json::json!({ "query": clause })
    }
}

impl Engine for Elasticsearch {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .ok_or_else(|| EngineError::Message("elasticsearch requires base_url".into()))?
            .trim_end_matches('/');
        let mut request = EngineRequest::post(format!(
            "{base}/{}/_search?from={}&size=10",
            self.index,
            pageno.saturating_sub(1) * 10
        ))
        .json_body(self.payload(query))
        .header("Content-Type", "application/json");
        if !self.username.is_empty() {
            // EngineRequest has no auth builder, so send the basic header.
            let credentials = format!("{}:{}", self.username, self.password);
            request = request.header(
                "Authorization",
                &format!("Basic {}", base64(credentials.as_bytes())),
            );
        }
        Ok(request)
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(hits) = json_path(&json, "hits.hits").and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for hit in hits {
            let source = json_path(hit, "_source").unwrap_or(hit);
            let url = ["url", "link"]
                .iter()
                .map(|key| json_str(source, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            let title = ["title", "name"]
                .iter()
                .map(|key| json_str(source, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            if !is_http_url(&url) || title.is_empty() {
                continue;
            }
            let content = ["content", "description", "text"]
                .iter()
                .map(|key| json_str(source, key))
                .find(|value| !value.is_empty())
                .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, strip_tags(&title))
                    .content(strip_tags(&content))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Standard base64 (RFC 4648) for the basic auth header.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let shift = 18 - index * 6;
                out.push(char::from(ALPHABET[((value >> shift) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Elasticsearch> {
        let mut config = EngineConfig::new("elasticsearch", "elasticsearch", "els", &["general"]);
        config.base_url = Some("http://localhost:9200".to_string());
        config
            .extra
            .insert("index".into(), toml::Value::String("pages".into()));
        Elasticsearch::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_an_index() {
        let config = EngineConfig::new("elasticsearch", "elasticsearch", "els", &["general"]);
        assert!(Elasticsearch::from_config(&config).is_err());
    }

    #[test]
    fn posts_to_the_search_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert_eq!(
            request.url,
            "http://localhost:9200/pages/_search?from=10&size=10"
        );
        let body = request.json.clone().unwrap();
        assert_eq!(body["query"]["match"]["_all"], serde_json::json!("rust"));
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }

    #[test]
    fn parses_the_hits() {
        let body = r##"{"hits":{"hits":[
            {"_source":{"url":"https://example.test/a","title":"A","content":"About A"}},
            {"_source":{"title":"no url"}}
        ]}}"##;
        let response = EngineResponse::new(200, "http://localhost:9200", body.as_bytes().to_vec());
        let query = Query::parse("a", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.test/a");
                assert_eq!(result.title, "A");
            }
            _ => panic!("expected a result"),
        }
    }
}
