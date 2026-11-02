//! Sogou engine, ported from searx/engines/sogou.py.
// engine-id: sogou

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::settings::EngineConfig;
use scraper::ElementRef;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "sogou",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.sogou.com";

/// Sogou web search.
pub struct Sogou {
    meta: EngineMeta,
}

impl Sogou {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Sogou {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let mut pairs = vec![("query", query.query_text()), ("page", pageno.to_string())];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "inttime_day",
                TimeRange::Week => "inttime_week",
                TimeRange::Month => "inttime_month",
                TimeRange::Year => "inttime_year",
            };
            pairs.push(("s_from", value.to_string()));
            pairs.push(("tsn", "1".to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{}/web?{}",
            base.trim_end_matches('/'),
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        if response.status == 302
            && response
                .header("location")
                .is_some_and(|location| location.contains("antispider"))
        {
            return Err(EngineError::Captcha);
        }
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        for item in select_elements(&html, "div.rb, div.vrwrap")? {
            let classes = attr(item, "class").unwrap_or_default();
            let is_wrap = classes.split_whitespace().any(|name| name == "vrwrap");
            if is_wrap && child(item, "div.special-wrap").is_some() {
                continue;
            }
            let parsed = if child(item, "h3.pt a").is_some() {
                parse_plain(item, base)
            } else {
                parse_with_image(item, base)
            };
            if let Some(result) = parsed {
                results.push(result);
            }
        }
        Ok(results)
    }
}

/// A plain result block: h3.pt / div.ft / cite.
fn parse_plain(item: ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let link = child(item, "h3.pt a")?;
    let title = text(link);
    let url = extract_url(&attr(link, "href").unwrap_or_default(), item, base);
    if title.is_empty() || !is_http_url(&url) {
        return None;
    }
    let content = text_of(item, "div.ft").unwrap_or_default();
    let date = parse_date(&text_of(item, "cite").unwrap_or_default());
    let builder = ResultBuilder::new(url, title)
        .content(content)
        .published_date(date.unwrap_or_default());
    Some(builder.build())
}

/// A result block that carries a thumbnail: h3.vr-title / img-layout.
fn parse_with_image(item: ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let link = child(item, "h3[class*='vr-title'] a")?;
    let title = text(link);
    let url = extract_url(&attr(link, "href").unwrap_or_default(), item, base);
    if title.is_empty() || !is_http_url(&url) {
        return None;
    }
    let content = text_of(item, "div[class*='attribute-centent']")
        .or_else(|| text_of(item, "div[class*='fz-mid']"))
        .unwrap_or_default();
    let date = parse_date(&text_of(item, "span.cite-date").unwrap_or_default());
    let thumbnail = child(item, "div[class*='img-layout'] img")
        .and_then(|image| attr(image, "src"))
        .map(|src| src.replace("http://", "https://"));
    let mut builder = ResultBuilder::new(url, title)
        .content(content)
        .published_date(date.unwrap_or_default());
    if let Some(thumbnail) = thumbnail {
        builder = builder.thumbnail(thumbnail);
    }
    Some(builder.build())
}

/// Upstream follows the target stored in data-url for /link? redirects.
fn extract_url(url: &str, item: ElementRef<'_>, base: &str) -> String {
    if !url.starts_with("/link?url=") {
        return url.to_string();
    }
    attr_of(item, "[data-url]", "data-url")
        .unwrap_or_else(|| format!("{}{url}", base.trim_end_matches('/')))
}

/// The first YYYY-MM-DD date of the text, reformatted with two digit fields.
fn parse_date(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '-');
        let parts: Vec<&str> = token.split('-').collect();
        if parts.len() != 3 {
            continue;
        }
        let (year, month, day) = (parts[0], parts[1], parts[2]);
        let digits = |value: &str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
        if year.len() != 4 || !digits(year) || !digits(month) || !digits(day) {
            continue;
        }
        let (Ok(year), Ok(month), Ok(day)) = (
            year.parse::<u32>(),
            month.parse::<u32>(),
            day.parse::<u32>(),
        ) else {
            continue;
        };
        return Some(format!("{year:04}-{month:02}-{day:02}"));
    }
    None
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine() -> Arc<Sogou> {
        Sogou::from_config(&EngineConfig::new("sogou", "sogou", "sogou", &["general"])).unwrap()
    }

    fn parse(html: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, BASE_URL, html.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        engine().parse(&response, &query).unwrap()
    }

    #[test]
    fn parses_the_plain_result_block() {
        let html = r##"<div class="rb"><h3 class="pt"><a href="/link?url=abc" data-url="https://example.com/alpha">Alpha</a></h3><div class="ft">Alpha <b>description</b></div><cite>2024-05-06 - example.com</cite></div>"##;
        match &parse(html)[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/alpha");
                assert_eq!(result.title, "Alpha");
                assert_eq!(result.content, "Alpha description");
                assert_eq!(result.published_date.as_deref(), Some("2024-05-06"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn parses_the_image_result_block_and_skips_special_wraps() {
        let html = r##"<div class="vrwrap"><h3 class="vr-title"><a href="/link?url=xyz" data-url="https://example.com/beta">Beta</a></h3><div class="attribute-centent">Beta summary</div><span class="cite-date">2023-12-01</span><div class="img-layout"><img src="http://img.example.com/b.jpg"/></div></div><div class="vrwrap"><div class="special-wrap">ad</div><h3 class="vr-title"><a href="https://example.com/ad">Ad</a></h3></div>"##;
        let items = parse(html);
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://example.com/beta");
                assert_eq!(result.title, "Beta");
                assert_eq!(result.content, "Beta summary");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/b.jpg")
                );
                assert_eq!(result.published_date.as_deref(), Some("2023-12-01"));
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn detects_the_antispider_redirect() {
        let mut response = EngineResponse::new(302, BASE_URL, Vec::new());
        response
            .headers
            .insert("location".into(), "http://www.sogou.com/antispider/".into());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine().parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn forwards_the_time_range() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("s_from=inttime_week"));
        assert!(request.url.contains("tsn=1"));
        assert!(request.url.contains("page=2"));
    }
}
