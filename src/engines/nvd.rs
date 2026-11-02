//! National Vulnerability Database engine, ported from searx/engines/nvd.py.
// engine-id: nvd

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["it"],
    shortcut: "nvd",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str =
    "https://nvd.nist.gov/extensions/nudp/services/json/nvd/cve/search/results";
const DETAIL_URL: &str = "https://nvd.nist.gov/vuln/detail/";
const REFERER: &str = "https://nvd.nist.gov/vuln/search";
const RESULTS_PER_PAGE: u32 = 10;

/// NVD CVE search.
pub struct Nvd {
    meta: EngineMeta,
}

impl Nvd {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> String {
        self.meta
            .base_url
            .clone()
            .unwrap_or_else(|| SEARCH_URL.to_string())
    }
}

impl Engine for Nvd {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = (pageno.max(1) - 1) * RESULTS_PER_PAGE;
        let pairs = vec![
            ("resultType", "records".to_string()),
            ("keyword", query.query_text()),
            ("rowCount", RESULTS_PER_PAGE.to_string()),
            ("offset", offset.to_string()),
        ];
        Ok(
            EngineRequest::get(format!("{}?{}", self.base_url(), query_string(&pairs)))
                .header("Accept", "application/json")
                .header("Referer", REFERER),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) =
            json_path(&json, "response.0.grid.vulnerabilities").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for item in items {
            let cve_id = json_str(item, "cve.id");
            if cve_id.is_empty() {
                continue;
            }
            let url = format!("{DETAIL_URL}{cve_id}");
            if !is_http_url(&url) {
                continue;
            }
            let published = date(&json_str(item, "cve.published"));
            let severity = json_str(item, "cve.metrics.cvssMetricV31.0.cvssData.baseSeverity");
            let score = json_str(item, "cve.metrics.cvssMetricV31.0.cvssData.baseScore");
            results.push(
                ResultBuilder::new(url, cve_id)
                    .content(json_str(item, "cve.descriptions.0.value"))
                    .published_date(published.unwrap_or_default())
                    .metadata("severity", severity)
                    .metadata("cvss_score", score)
                    .build(),
            );
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Upstream parses the published field with datetime.fromisoformat.
fn date(value: &str) -> Option<String> {
    let value = value.trim();
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(date.to_rfc3339());
    }
    if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(date.format("%Y-%m-%dT%H:%M:%S").to_string());
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%dT00:00:00").to_string())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"response":[{"grid":{"vulnerabilities":[
      {"cve":{"id":"CVE-2023-1234","published":"2023-08-01T10:15:00.000","descriptions":[{"lang":"en","value":"A buffer overflow."}],"metrics":{"cvssMetricV31":[{"cvssData":{"baseScore":7.5,"baseSeverity":"HIGH"}}]}}},
      {"cve":{"id":"CVE-2023-5678","published":"2023-09-02","descriptions":[{"lang":"en","value":"An issue."}],"metrics":{}}},
      {"cve":{"id":"","descriptions":[]}}
    ]}}]}"##;

    fn engine() -> Arc<Nvd> {
        Nvd::from_config(&EngineConfig::new(
            "national vulnerability database",
            "nvd",
            "nvd",
            &["it"],
        ))
        .unwrap()
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_vulnerabilities() {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://nvd.nist.gov/vuln/detail/CVE-2023-1234");
        assert_eq!(result.title, "CVE-2023-1234");
        assert_eq!(result.content, "A buffer overflow.");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2023-08-01T10:15:00")
        );
        assert_eq!(
            result.metadata.get("severity").map(String::as_str),
            Some("HIGH")
        );
        assert_eq!(
            result.metadata.get("cvss_score").map(String::as_str),
            Some("7.5")
        );
        let EngineItem::Result(second) = &items[1] else {
            panic!("expected a result")
        };
        assert_eq!(
            second.published_date.as_deref(),
            Some("2023-09-02T00:00:00")
        );
        assert_eq!(second.metadata.get("severity"), None);
    }

    #[test]
    fn builds_the_search_request() {
        let request = engine().build_request(&query("rust"), 2).unwrap();
        for part in [
            "resultType=records",
            "keyword=rust",
            "rowCount=10",
            "offset=10",
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
        assert_eq!(referer, Some(REFERER));
    }

    #[test]
    fn a_missing_container_yields_no_results() {
        let response = EngineResponse::new(200, SEARCH_URL, b"{}".to_vec());
        assert!(
            engine()
                .parse(&response, &query("rust"))
                .unwrap()
                .is_empty()
        );
    }
}
