//! BT4G engine, ported from searx/engines/bt4g.py.
// engine-id: bt4g

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;
use quick_xml::events::Event;
const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "bt4g",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};
const BASE_URL: &str = "https://bt4gprx.com";
fn is_http(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}
#[derive(Default)]
struct Entry {
    title: String,
    guid: String,
    link: String,
    description: String,
    pub_date: String,
}
/// Read the RSS items. Upstream uses guid as the result URL and link as the
/// magnet link, which the shared RSS helper does not keep apart.
fn parse_feed(xml: &str) -> Result<Vec<Entry>, EngineError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buffer = Vec::new();
    let mut entries = Vec::new();
    let mut entry: Option<Entry> = None;
    let mut field: Option<&'static str> = None;
    let mut captured = String::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| EngineError::Parse(format!("invalid RSS response: {error}")))?;
        match event {
            Event::Start(start) => {
                let name = start.local_name().as_ref().to_ascii_lowercase();
                field = match name.as_slice() {
                    b"item" => {
                        entry = Some(Entry::default());
                        None
                    }
                    b"title" if entry.is_some() => Some("title"),
                    b"guid" if entry.is_some() => Some("guid"),
                    b"link" if entry.is_some() => Some("link"),
                    b"description" if entry.is_some() => Some("description"),
                    b"pubdate" if entry.is_some() => Some("pub_date"),
                    _ => None,
                };
                if field.is_some() {
                    captured.clear();
                }
            }
            Event::Text(text) => {
                if field.is_some() {
                    captured.push_str(&decode_entities(&String::from_utf8_lossy(
                        &text.into_inner(),
                    )));
                }
            }
            Event::CData(data) => {
                if field.is_some() {
                    captured.push_str(&String::from_utf8_lossy(&data.into_inner()));
                }
            }
            Event::End(end) => {
                if end.local_name().as_ref().eq_ignore_ascii_case(b"item") {
                    if let Some(item) = entry.take() {
                        entries.push(item);
                    }
                } else if let (Some(item), Some(name)) = (entry.as_mut(), field) {
                    let value = std::mem::take(&mut captured);
                    match name {
                        "title" => item.title = clean(&value),
                        "guid" => item.guid = value.trim().to_string(),
                        "link" => item.link = value.trim().to_string(),
                        "description" => item.description = value.trim().to_string(),
                        "pub_date" => item.pub_date = value.trim().to_string(),
                        _ => {}
                    }
                }
                field = None;
                captured.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(entries)
}
/// BT4G, the RSS search feed.
pub struct Bt4g {
    meta: EngineMeta,
}
impl Bt4g {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}
impl Engine for Bt4g {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }
    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        let base = base.trim_end_matches('/');
        let order_by = self.meta.opt_str("bt4g_order_by").unwrap_or("relevance");
        let order_by = if query.time_range.is_some() {
            "time"
        } else {
            order_by
        };
        let category = self.meta.opt_str("bt4g_category").unwrap_or("all");
        let pairs: Vec<(&str, String)> = vec![
            ("q", query.query_text()),
            ("orderby", order_by.to_string()),
            ("category", category.to_string()),
            ("p", pageno.max(1).to_string()),
            ("page", "rss".to_string()),
        ];
        let url = format!("{base}/search?{}", query_string(&pairs));
        Ok(EngineRequest::get(url))
    }
    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let entries = parse_feed(&response.text())?;
        let mut results = Vec::new();
        for entry in entries {
            let url = entry.guid.trim().to_string();
            if entry.title.is_empty() || !is_http(&url) {
                continue;
            }
            let filesize = entry
                .description
                .split("<br>")
                .nth(1)
                .map(str::trim)
                .unwrap_or_default();
            let published =
                chrono::DateTime::parse_from_str(entry.pub_date.trim(), "%a,%d %b %Y %H:%M:%S %z")
                    .map(|date| date.with_timezone(&chrono::Utc).to_rfc3339())
                    .unwrap_or_default();
            results.push(
                ResultBuilder::new(url, entry.title.clone())
                    .metadata("seed", "N/A")
                    .metadata("leech", "N/A")
                    .metadata("filesize", filesize)
                    .metadata("magnetlink", entry.link.trim())
                    .published_date(published)
                    .build(),
            );
        }
        Ok(results)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::TimeRange;
    const FIXTURE: &str = r##"<rss version="2.0"><channel>
      <item><title>Ubuntu 24.04</title><guid>https://bt4gprx.com/magnet/abc</guid><link>magnet:?xt=urn:btih:ABC</link><description><![CDATA[ubuntu.iso<br>5.8 GB<br>files]]></description><pubDate>Sat,24 Oct 2026 10:00:00 +0800</pubDate></item>
      <item><title>Broken</title><guid>not-a-url</guid><link>magnet:?xt=urn:btih:DEF</link><description>no size</description></item>
    </channel></rss>"##;
    fn engine() -> Arc<Bt4g> {
        let config = EngineConfig::new("bt4g", "bt4g", "bt4g", &["files"]);
        Bt4g::from_config(&config).unwrap()
    }

    #[test]
    fn parses_the_rss_feed() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result");
        };
        assert_eq!(result.url, "https://bt4gprx.com/magnet/abc");
        assert_eq!(result.title, "Ubuntu 24.04");
        assert_eq!(result.metadata["filesize"], "5.8 GB");
        assert_eq!(result.metadata["magnetlink"], "magnet:?xt=urn:btih:ABC");
        assert!(result.published_date.is_some());
    }
    #[test]
    fn builds_the_search_url_with_config() {
        let mut config = EngineConfig::new("bt4g", "bt4g", "bt4g", &["files"]);
        config
            .extra
            .insert("bt4g_order_by".into(), "seeders".into());
        config.extra.insert("bt4g_category".into(), "movie".into());
        let engine = Bt4g::from_config(&config).unwrap();
        let mut query = Query::parse("ubuntu", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(request.url.contains("orderby=seeders"));
        assert!(request.url.contains("category=movie"));
        query.time_range = Some(TimeRange::Week);
        let request = engine.build_request(&query, 1).unwrap();
        assert!(request.url.contains("orderby=time"));
    }
}
