//! Acfun engine, ported from searx/engines/acfun.py.

// engine-id: acfun

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "acf",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.acfun.cn";
/// The bigPipe payloads that carry the search markup (upstream regex).
const BIGPIPE_RE: &str = "(?s)bigPipe\\.onPageletArrive\\((\\{.*?\\})\\);";

/// Acfun video search.
pub struct Acfun {
    meta: EngineMeta,
}

impl Acfun {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for Acfun {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = base_url(&self.meta);
        let pairs = vec![
            ("keyword", query.query_text()),
            ("pCursor", pageno.to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{base}/search?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let pattern = regex::Regex::new(BIGPIPE_RE)
            .map_err(|error| EngineError::Parse(format!("invalid bigPipe pattern: {error}")))?;
        let base = base_url(&self.meta);
        let mut results = Vec::new();
        for captures in pattern.captures_iter(&body) {
            let Some(payload) = captures.get(1) else {
                continue;
            };
            // A broken payload is skipped, exactly like upstream.
            let Ok(json) = serde_json::from_str::<serde_json::Value>(payload.as_str()) else {
                continue;
            };
            let raw_html = json_str(&json, "html");
            if raw_html.is_empty() {
                continue;
            }
            let fragment = parse_html(&raw_html);
            for block in select_elements(&fragment, "div[class*='search-video']")? {
                if let Some(item) = video_item(block, &base) {
                    results.push(item);
                }
            }
        }
        Ok(results)
    }
}

fn base_url(meta: &EngineMeta) -> String {
    meta.base_url
        .as_deref()
        .unwrap_or(BASE_URL)
        .trim_end_matches('/')
        .to_string()
}

/// One search-video block, upstream extract_video_data().
fn video_item(block: scraper::ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let exposure = attr(block, "data-exposure-log")?;
    let data: serde_json::Value = serde_json::from_str(&exposure).ok()?;
    let content_id = json_str(&data, "content_id");
    let title = json_str(&data, "title");
    if content_id.is_empty() || title.is_empty() {
        return None;
    }
    let url = format!("{base}/v/ac{content_id}");
    if !is_http_url(&url) {
        return None;
    }
    let iframe_src = format!("{base}/player/ac{content_id}");
    let intro = text_of(block, "div[class*='video__main__intro']").unwrap_or_default();
    let duration = text_of(block, "span[class*='video__duration']").unwrap_or_default();
    let created = text_of(block, "span[class*='info__create-time']").unwrap_or_default();
    let cover = child(block, "div[class*='video__cover']")
        .and_then(|cover| child(cover, "img"))
        .and_then(|image| attr(image, "src"))
        .and_then(|src| absolute_url(&src, base));
    let mut builder = ResultBuilder::new(url, title)
        .content(intro)
        .template(Template::Videos)
        .metadata("iframe_src", iframe_src);
    if let Some(cover) = cover {
        builder = builder.thumbnail(cover);
    }
    if let Some(date) = iso_date(&created) {
        builder = builder.published_date(date);
    }
    if !duration.is_empty() {
        builder = builder.metadata("length", duration);
    }
    Some(builder.build())
}

/// Upstream parses the create time with datetime.fromisoformat().
fn iso_date(value: &str) -> Option<String> {
    let value = value.trim();
    let date = value.get(..10)?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(value.to_string())
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<html><script>
bigPipe.onPageletArrive({"html": "<div class=\"search-video\" data-exposure-log='{\"content_id\":\"123\",\"title\":\"Rust 2024\"}'><div class=\"video__cover\"><a href=\"/v/ac123\"><img src=\"//static.acfun.cn/cover.jpg\"></a></div><span class=\"info__create-time\">2024-05-01 12:00:00</span><span class=\"video__duration\">03:21</span><div class=\"video__main__intro\">The intro</div></div>"});
</script></html>"##;

    fn engine() -> Arc<Acfun> {
        Acfun::from_config(&EngineConfig::new("acfun", "acfun", "acf", &["videos"])).unwrap()
    }

    fn query() -> Query {
        Query::parse("rust", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_the_bigpipe_payload() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let items = engine().parse(&response, &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Rust 2024");
                assert_eq!(result.url, "https://www.acfun.cn/v/ac123");
                assert_eq!(result.content, "The intro");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://static.acfun.cn/cover.jpg")
                );
                assert_eq!(result.template, Template::Videos);
                assert_eq!(
                    result.published_date.as_deref(),
                    Some("2024-05-01 12:00:00")
                );
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("03:21")
                );
                assert_eq!(
                    result.metadata.get("iframe_src").map(String::as_str),
                    Some("https://www.acfun.cn/player/ac123")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_search_request() {
        let request = engine().build_request(&query(), 2).unwrap();
        assert_eq!(
            request.url,
            "https://www.acfun.cn/search?keyword=rust&pCursor=2"
        );
    }

    #[test]
    fn missing_payload_yields_no_results() {
        let response = EngineResponse::new(200, BASE_URL, b"<html></html>".to_vec());
        assert!(engine().parse(&response, &query()).unwrap().is_empty());
    }
}
