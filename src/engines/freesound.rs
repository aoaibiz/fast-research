//! Freesound engine, ported from searx/engines/freesound.py.
// engine-id: freesound

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "fnd",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 15.0,
};

/// Upstream builds its search_url from this API root.
const BASE_URL: &str = "https://freesound.org/apiv2";
const FIELDS: &str = "name,url,download,created,description,type";
const MAX_DESCRIPTION: usize = 128;

/// Freesound, the Creative Commons sound archive. Requires an api_key.
pub struct Freesound {
    meta: EngineMeta,
}

impl Freesound {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn api_root(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    fn api_key(&self) -> Result<&str, EngineError> {
        self.meta
            .api_key
            .as_deref()
            .or_else(|| self.meta.opt_str("api_key"))
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| EngineError::Message("freesound requires an api_key".into()))
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// The created timestamp of a sound as ISO 8601, empty when unparsable.
fn iso_date(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return date.to_rfc3339();
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return date.and_utc().to_rfc3339();
        }
    }
    String::new()
}

/// Upstream keeps only the first 128 characters of the description.
fn short_description(value: &str) -> String {
    value.chars().take(MAX_DESCRIPTION).collect()
}

impl Engine for Freesound {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let key = self.api_key()?;
        // Upstream formats urlencode({"q": query}) into the query parameter,
        // which sends a literal "q=" prefix; this port sends the query itself.
        let pairs: Vec<(&str, String)> = vec![
            ("query", query.query_text()),
            ("page", pageno.max(1).to_string()),
            ("fields", FIELDS.to_string()),
            ("token", key.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{}/search/text/?{}",
            self.api_root(),
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
        let items = match json_path(&json, "results").and_then(|value| value.as_array()) {
            Some(items) => items,
            None => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        for item in items {
            let url = json_str(item, "url");
            let title = json_str(item, "name");
            if url.is_empty() || title.is_empty() || !is_http(&url) {
                continue;
            }
            let download = json_str(item, "download");
            let mut builder = ResultBuilder::new(url, title)
                .content(short_description(&json_str(item, "description")))
                .published_date(iso_date(&json_str(item, "created")));
            if is_http(&download) {
                builder = builder.metadata("audio_src", download);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"results": [
      {
        "name": "Rain on a tin roof",
        "url": "https://freesound.org/people/klankbeeld/sounds/123456/",
        "download": "https://freesound.org/apiv2/sounds/123456/download/",
        "created": "2016-01-20T10:31:12.506",
        "description": "Recorded on a rainy night under a tin roof."
      },
      {"name": "No url", "url": "", "created": "2016-01-20T10:31:12.506"}
    ]}"##;

    fn engine(key: Option<&str>) -> Arc<Freesound> {
        let mut config = EngineConfig::new("freesound", "freesound", "fnd", &["general"]);
        config.api_key = key.map(str::to_string);
        Freesound::from_config(&config).expect("freesound engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn builds_the_paged_search_url() {
        let request = engine(Some("KEY"))
            .build_request(&query("rain"), 2)
            .expect("request");
        assert_eq!(request.method, crate::engine::Method::Get);
        assert!(
            request
                .url
                .starts_with("https://freesound.org/apiv2/search/text/?query=rain&page=2&fields=")
        );
        assert!(request.url.ends_with("token=KEY"));
        assert!(matches!(
            engine(None).build_request(&query("rain"), 1),
            Err(EngineError::Message(_))
        ));
        assert_eq!(engine(Some("KEY")).api_root(), BASE_URL);
    }

    #[test]
    fn parses_the_sounds() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine(Some("KEY"))
            .parse(&response, &query("rain"))
            .expect("items");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rain on a tin roof");
                assert_eq!(
                    result.url,
                    "https://freesound.org/people/klankbeeld/sounds/123456/"
                );
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2016-01-20T10:31:12.506+00:00")
                );
                assert_eq!(
                    result.metadata.get("audio_src").map(String::as_str),
                    Some("https://freesound.org/apiv2/sounds/123456/download/")
                );
                assert_eq!(
                    result.content,
                    "Recorded on a rainy night under a tin roof."
                );
            }
            _ => panic!("expected a result"),
        }
        let long = "a".repeat(300);
        assert_eq!(short_description(&long).chars().count(), MAX_DESCRIPTION);
        let empty = EngineResponse::new(200, BASE_URL, b"{}".to_vec());
        let items = engine(Some("KEY")).parse(&empty, &query("rain"));
        assert!(items.expect("items").is_empty());
    }
}
