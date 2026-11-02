//! Yahoo News engine, ported from searx/engines/yahoo_news.py.
//!
//! Yahoo News is English only and does not accept language queries. Relative
//! publication dates such as 3 hours ago are converted to a timestamp here.
// engine-id: yahoo_news

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["news"],
    shortcut: "yhn",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str = "https://news.search.yahoo.com/search";

/// Yahoo News, through the HTML search page.
pub struct YahooNews {
    meta: EngineMeta,
}

impl YahooNews {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
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
    let end = ["/RS", "/RK"]
        .iter()
        .filter_map(|ending| raw.rfind(*ending))
        .min();
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

/// The number and the unit of a relative date such as 3 hours ago.
fn ago_delta(raw: &str) -> Option<(i32, Duration)> {
    let lower = raw.to_lowercase();
    let start = lower.find(|character: char| character.is_ascii_digit())?;
    let rest = &lower[start..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let number: i32 = digits.parse().ok()?;
    let unit: String = lower[start + digits.len()..]
        .trim_start()
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect();
    // The upstream regex matches the singular prefix of the plural words.
    let unit = unit.trim_end_matches('s');
    let delta = match unit {
        "minute" => Duration::minutes(1),
        "hour" => Duration::hours(1),
        "day" => Duration::days(1),
        "week" => Duration::days(7),
        "month" => Duration::days(30),
        "year" => Duration::days(365),
        _ => return None,
    };
    Some((number, delta))
}

fn relative_date(raw: &str) -> Option<DateTime<Utc>> {
    let (number, delta) = ago_delta(raw)?;
    Utc::now().checked_sub_signed(delta.checked_mul(number)?)
}

/// RFC 3339 and RFC 2822 cover the absolute dates Yahoo publishes.
fn parse_absolute(raw: &str) -> Option<DateTime<Utc>> {
    let value = raw.trim();
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_rfc2822(value))
        .map(|date| date.with_timezone(&Utc))
        .ok()
}

fn published_date(raw: &str) -> String {
    relative_date(raw)
        .or_else(|| parse_absolute(raw))
        .map(|date| date.to_rfc3339())
        .unwrap_or_default()
}

impl Engine for YahooNews {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let offset = (pageno.max(1) - 1) * 10 + 1;
        let pairs: Vec<(&str, String)> = vec![("p", query.query_text()), ("b", offset.to_string())];
        Ok(EngineRequest::get(format!(
            "{SEARCH_URL}?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut items = Vec::new();
        for element in select_elements(&html, "ol[class*='searchCenterMiddle'] li")? {
            let Some(link) = child(element, "h4 a") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let url = parse_url(&href);
            if !is_http_url(&url) {
                continue;
            }
            let title = strip_tags(&text(link));
            if title.is_empty() {
                continue;
            }
            let content = text_of(element, "p").unwrap_or_default();
            let mut builder = ResultBuilder::new(url, title).content(content);
            if let Some(thumbnail) = attr_of(element, "img", "data-src") {
                builder = builder.thumbnail(thumbnail);
            }
            let stamp = text_of(element, "span[class*='s-time']").unwrap_or_default();
            items.push(builder.published_date(published_date(&stamp)).build());
        }
        for element in select_elements(&html, "div[class*='AlsoTry'] td")? {
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

    const FIXTURE: &str = r##"<html><body>
      <ol class="searchCenterMiddle">
        <li>
          <h4><a href="https://r.search.yahoo.com/_ylt=abc/RU=https%3A%2F%2Fwww.rust-lang.org%2Fnews/RS=def">Rust news</a></h4>
          <p>A language empowering everyone.</p>
          <img data-src="https://img.example.com/rust.jpg">
          <span class="s-time">3 hours ago</span>
        </li>
        <li><p>No link here</p></li>
      </ol>
      <div class="AlsoTry"><table><tr><td>rust async</td></tr></table></div>
    </body></html>"##;

    fn engine() -> Arc<YahooNews> {
        let config = EngineConfig::new("yahoo news", "yahoo_news", "yhn", &["news"]);
        YahooNews::from_config(&config).expect("yahoo news engine")
    }

    fn query(text: &str) -> Query {
        Query::parse(text, &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_news_results_and_suggestions() {
        let response = EngineResponse::new(
            200,
            "https://news.search.yahoo.com/",
            FIXTURE.as_bytes().to_vec(),
        );
        let items = engine().parse(&response, &query("rust")).unwrap();
        assert_eq!(items.len(), 2);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.url, "https://www.rust-lang.org/news");
                assert_eq!(result.title, "Rust news");
                assert_eq!(result.content, "A language empowering everyone.");
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://img.example.com/rust.jpg")
                );
                assert!(
                    result
                        .published_date
                        .as_deref()
                        .is_some_and(|value| !value.is_empty())
                );
            }
            other => panic!("expected a result, got {other:?}"),
        }
        match &items[1] {
            EngineItem::Suggestion(suggestion) => assert_eq!(suggestion, "rust async"),
            other => panic!("expected a suggestion, got {other:?}"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let request = engine().build_request(&query("rust news"), 3).unwrap();
        assert_eq!(
            request.url,
            "https://news.search.yahoo.com/search?p=rust%20news&b=21"
        );
    }

    #[test]
    fn dates_are_relative_or_absolute() {
        assert!(relative_date("3 hours ago").is_some());
        assert!(relative_date("2 weeks ago").is_some());
        assert!(relative_date("yesterday").is_none());
        assert_eq!(
            published_date("2024-05-01T12:00:00Z"),
            "2024-05-01T12:00:00+00:00"
        );
        assert!(published_date("").is_empty());
    }
}
