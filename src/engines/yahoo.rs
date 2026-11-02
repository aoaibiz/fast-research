//! Yahoo engine, ported from searx/engines/yahoo.py (web search).
//!
//! The sB cookie carries language and safesearch; YBV is not handled here.
// engine-id: yahoo

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general", "web"],
    shortcut: "yh",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: true,
    weight: 1.0,
    timeout: 4.0,
};

/// Regions with a dedicated Yahoo domain (upstream region2domain).
const REGIONS: &[&str] = &[
    "AR", "AT", "AU", "BE", "BR", "CA", "CH", "CL", "CO", "DE", "DK", "ES", "FI", "FR", "GB", "GR",
    "ID", "IE", "IN", "IT", "MX", "MY", "NL", "NO", "NZ", "PE", "PH", "PL", "RO", "SE", "SG", "TH",
    "TR", "UK", "US", "VE", "VN", "ZA",
];

/// Languages accepted by Yahoo in the sB cookie (zh is spelled zh_chs).
const LANGS: &[&str] = &[
    "ar", "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "he", "hr", "hu", "it", "ja",
    "ko", "lt", "lv", "nl", "no", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "th", "tr",
];

/// Yahoo web search.
pub struct Yahoo {
    meta: EngineMeta,
}

impl Yahoo {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

fn domain(region: &str) -> String {
    match region {
        "GB" | "UK" => "uk.search.yahoo.com".to_string(),
        "ES" => "espanol.search.yahoo.com".to_string(),
        "MY" => "malaysia.search.yahoo.com".to_string(),
        "US" => "search.yahoo.com".to_string(),
        other if REGIONS.contains(&other) => format!("{}.search.yahoo.com", other.to_lowercase()),
        _ => "search.yahoo.com".to_string(),
    }
}

/// Remove the Yahoo tracking part of a redirect URL (upstream parse_url).
fn parse_url(raw: &str) -> String {
    let Some(marker) = raw.find("/RU=") else {
        return raw.to_string();
    };
    let Some(offset) = raw[marker + 1..].find("http") else {
        return raw.to_string();
    };
    let start = marker + 1 + offset;
    let mut end: Option<usize> = None;
    for ending in ["/RS", "/RK"] {
        if let Some(position) = raw.rfind(ending) {
            end = Some(end.map_or(position, |current| current.min(position)));
        }
    }
    let Some(end) = end else {
        return raw.to_string();
    };
    if end <= start {
        return raw.to_string();
    }
    percent_encoding::percent_decode_str(&raw[start..end])
        .decode_utf8()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| raw[start..end].to_string())
}

impl Engine for Yahoo {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let code = locale_language(&query.locale);
        let language = match code.as_str() {
            "zh" => "zh_chs".to_string(),
            other if LANGS.contains(&other) => other.to_string(),
            _ => "any".to_string(),
        };
        let mut pairs: Vec<(&str, String)> = vec![("p", query.query_text())];
        let btf = match query.time_range {
            Some(TimeRange::Day) => "d",
            Some(TimeRange::Week) => "w",
            Some(TimeRange::Month) => "m",
            _ => "",
        };
        if !btf.is_empty() {
            pairs.push(("btf", btf.to_string()));
        }
        if pageno == 1 {
            pairs.push(("iscqry", String::new()));
        } else {
            pairs.push(("b", (pageno * 7 + 1).to_string()));
            pairs.push(("pz", "7".to_string()));
            pairs.push(("bct", "0".to_string()));
            pairs.push(("xargs", "0".to_string()));
        }
        let safe = match query.safesearch {
            0 => "p",
            1 => "i",
            _ => "r",
        };
        let sb = format!("v=1&vm={safe}&fl=1&vl=lang_{language}&pn=10&rw=new&userset=1");
        let region = locale_region(&query.locale).to_uppercase();
        let url = format!(
            "https://{}/search?{}",
            domain(&region),
            query_string(&pairs)
        );
        Ok(EngineRequest::get(url).cookie("sB", &sb))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut items = Vec::new();
        for element in select_elements(&html, "div[class*='algo-sr']")? {
            let Some(href) = attr_of(element, "div[class*='compTitle'] a", "href") else {
                continue;
            };
            let url = parse_url(&href);
            if url.is_empty() || !url.starts_with("http") {
                continue;
            }
            let title = attr_of(element, "h3 a", "aria-label")
                .or_else(|| text_of(element, "a h3"))
                .map(|value| strip_tags(&value))
                .unwrap_or_default();
            if title.is_empty() {
                continue;
            }
            let content = text_of(element, "div[class*='compText']")
                .map(|value| strip_tags(&value))
                .unwrap_or_default();
            items.push(ResultBuilder::new(url, title).content(content).build());
        }
        for element in select_elements(&html, "div[class*='AlsoTry'] table a")? {
            let suggestion = strip_tags(&text(element));
            if !suggestion.is_empty() {
                items.push(EngineItem::Suggestion(suggestion));
            }
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EngineRegistry;

    fn engine() -> Arc<Yahoo> {
        Yahoo::from_config(&EngineConfig::new("yahoo", "yahoo", "yh", &["general"]))
            .expect("engine")
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_results_and_suggestions() {
        let html = r##"<html><body><div class="algo-sr">
          <div class="compTitle"><a href="https://r.search.yahoo.com/_ylt=x/RU=https%3A%2F%2Fwww.rust-lang.org%2F/RS=y">Rust</a></div>
          <h3><a aria-label="Rust">Rust</a></h3>
          <div class="compText">A language empowering everyone.</div>
        </div>
        <div class="AlsoTry"><table><tr><td><a>rust async</a></td></tr></table></div></body></html>"##;
        let response =
            EngineResponse::new(200, "https://search.yahoo.com/", html.as_bytes().to_vec());
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/");
                assert_eq!(result.title, "Rust");
                assert_eq!(result.content, "A language empowering everyone.");
            }
            _ => panic!("expected a result"),
        }
        match &items[1] {
            EngineItem::Suggestion(value) => assert_eq!(value, "rust async"),
            _ => panic!("expected a suggestion"),
        }
    }

    #[test]
    fn request_carries_locale_safesearch_and_time_range() {
        let mut request_query = query("rust");
        request_query.locale = "ja-JP".to_string();
        request_query.safesearch = 2;
        request_query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&request_query, 2).unwrap();
        assert!(request.url.starts_with("https://search.yahoo.com/search?"));
        assert!(request.url.contains("p=rust"));
        assert!(request.url.contains("btf=w"));
        assert!(request.url.contains("b=15"));
        assert!(request.url.contains("pz=7"));
        let sb = "v=1&vm=r&fl=1&vl=lang_ja&pn=10&rw=new&userset=1";
        assert_eq!(request.cookies[0], ("sB".to_string(), sb.to_string()));
        let mut german = query("rust");
        german.locale = "de-DE".to_string();
        let request = engine().build_request(&german, 1).unwrap();
        assert!(
            request
                .url
                .starts_with("https://de.search.yahoo.com/search?")
        );
    }

    #[test]
    fn tracking_urls_are_unwrapped() {
        let wrapped = "https://r.search.yahoo.com/_ylt=a/RU=https%3A%2F%2Fexample.com%2F/RK=2/RS=b";
        assert_eq!(parse_url(wrapped), "https://example.com/");
        assert_eq!(parse_url("https://example.com/"), "https://example.com/");
        let empty =
            EngineResponse::new(200, "https://search.yahoo.com/", b"<html></html>".to_vec());
        assert!(engine().parse(&empty, &query("rust")).unwrap().is_empty());
    }
}
