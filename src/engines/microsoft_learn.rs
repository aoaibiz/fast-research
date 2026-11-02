//! Microsoft Learn engine, ported from searx/engines/microsoft_learn.py.

// engine-id: microsoft_learn

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it"],
    shortcut: "msl",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_API: &str = "https://learn.microsoft.com/api/search";
const PAGE_SIZE: u32 = 10;

/// Microsoft Learn, Microsoft's technical knowledge base.
pub struct MicrosoftLearn {
    meta: EngineMeta,
}

impl MicrosoftLearn {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

/// The locale the API expects, en-us when no language was requested.
fn locale(query: &Query) -> String {
    let language = query.engine_language();
    match language.as_str() {
        // The API wants a locale (en-us), while the query gives a language (en).
        "" | "all" | "en" => "en-us".to_string(),
        other => other.to_string(),
    }
}

impl Engine for MicrosoftLearn {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs: Vec<(&str, String)> = vec![
            ("search", query.query_text()),
            ("locale", locale(query)),
            ("scoringprofile", "semantic-answers".to_string()),
            ("facet", "category".to_string()),
            ("facet", "products".to_string()),
            ("facet", "tags".to_string()),
            ("$top", PAGE_SIZE.to_string()),
            ("$skip", (pageno.saturating_sub(1) * PAGE_SIZE).to_string()),
            ("expandScope", "true".to_string()),
            ("includeQuestion", "false".to_string()),
            ("applyOperator", "false".to_string()),
            ("partnerId", "LearnSite".to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{SEARCH_API}?{}", query_string(&pairs)))
                .header("Accept", "application/json"),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "results").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let title = json_str(item, "title");
            let url = json_str(item, "url");
            if title.is_empty() || !is_http(&url) {
                continue;
            }
            results.push(
                ResultBuilder::new(url, title)
                    .content(json_str(item, "description"))
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{
      "results": [
        {"url": "https://learn.microsoft.com/en-us/dotnet/", "title": "C# documentation",
         "description": "The C# guide"},
        {"url": "javascript:alert(1)", "title": "Bad"},
        {"url": "https://learn.microsoft.com/en-us/empty", "title": ""}
      ]
    }"##;

    fn engine() -> Arc<MicrosoftLearn> {
        MicrosoftLearn::from_config(&EngineConfig::new(
            "microsoft learn",
            "microsoft_learn",
            "msl",
            &["it"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_learn_results() {
        let response = EngineResponse::new(200, SEARCH_API, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("csharp")).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "C# documentation");
                assert_eq!(result.url, "https://learn.microsoft.com/en-us/dotnet/");
                assert_eq!(result.content, "The C# guide");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_search_url() {
        let request = engine().build_request(&query("csharp"), 3).unwrap();
        assert!(request.url.starts_with(SEARCH_API));
        assert!(request.url.contains("search=csharp"));
        assert!(request.url.contains("locale=en-us"));
        assert!(request.url.contains("facet=category"));
        assert!(request.url.contains("%24top=10"));
        assert!(request.url.contains("%24skip=20"));
        assert!(request.url.contains("partnerId=LearnSite"));
    }

    #[test]
    fn language_is_forwarded() {
        let mut query = query("csharp");
        query.locale = "ja-JP".to_string();
        let request = engine().build_request(&query, 1).unwrap();
        assert!(request.url.contains("locale=ja-JP"));
    }

    #[test]
    fn missing_results_are_empty() {
        let response = EngineResponse::new(200, SEARCH_API, b"{}".to_vec());
        let items = engine().parse(&response, &query("csharp")).unwrap();
        assert!(items.is_empty());
    }
}
