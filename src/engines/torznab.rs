//! Torznab engine, ported from searx/engines/torznab.py.
// engine-id: torznab

use std::collections::BTreeMap;
use std::sync::Arc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 3.0,
};

#[derive(Default)]
struct XmlItem {
    fields: BTreeMap<String, String>,
}

impl XmlItem {
    fn get(&self, key: &str) -> &str {
        self.fields.get(key).map(String::as_str).unwrap_or_default()
    }
}

fn attribute(start: &BytesStart<'_>, name: &[u8]) -> String {
    let found = start
        .attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name);
    found
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Explicit1_0).ok())
        .map(|v| v.to_string())
        .unwrap_or_default()
}

fn humanize_bytes(mut size: f64) -> String {
    const SUFFIX: [&str; 5] = ["B ", "KB", "MB", "GB", "TB"];
    let mut position = 0usize;
    while size > 1024.0 && position < 4 {
        position += 1;
        size /= 1024.0;
    }
    format!("{size:.2} {}", SUFFIX[position])
}

fn pick(item: &XmlItem, keys: &[&str], prefix: &str) -> String {
    keys.iter()
        .map(|key| item.get(key))
        .find(|value| value.starts_with(prefix))
        .unwrap_or_default()
        .to_string()
}

fn parse_items(body: &str) -> Result<Vec<XmlItem>, EngineError> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut items = Vec::new();
    let mut item: Option<XmlItem> = None;
    let mut field: Option<&'static str> = None;
    let mut captured = String::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| EngineError::Parse(format!("invalid XML response: {error}")))?;
        match event {
            Event::Start(start) | Event::Empty(start) => {
                match start.local_name().as_ref() {
                    b"error" => {
                        return Err(EngineError::Message(attribute(&start, b"description")));
                    }
                    b"item" => item = Some(XmlItem::default()),
                    b"enclosure" => {
                        if let Some(entry) = item.as_mut() {
                            entry
                                .fields
                                .insert("enclosure_url".into(), attribute(&start, b"url"));
                            entry
                                .fields
                                .insert("enclosure_length".into(), attribute(&start, b"length"));
                        }
                    }
                    b"attr" => {
                        if let Some(entry) = item.as_mut() {
                            let value = attribute(&start, b"value");
                            entry.fields.insert(attribute(&start, b"name"), value);
                        }
                    }
                    b"title" => field = Some("title"),
                    b"guid" => field = Some("guid"),
                    b"comments" => field = Some("comments"),
                    b"link" => field = Some("link"),
                    b"pubDate" | b"pubdate" => field = Some("pubDate"),
                    b"size" => field = Some("size"),
                    _ => field = None,
                }
                captured.clear();
            }
            Event::Text(text) => {
                if field.is_some() {
                    if let Ok(value) = text.xml_content(quick_xml::XmlVersion::Explicit1_0) {
                        captured.push_str(&value);
                    }
                }
            }
            Event::End(end) => {
                if end.local_name().as_ref() == b"item" {
                    if let Some(entry) = item.take() {
                        items.push(entry);
                    }
                } else if let (Some(entry), Some(name)) = (item.as_mut(), field) {
                    entry
                        .fields
                        .insert(name.to_string(), std::mem::take(&mut captured));
                }
                field = None;
                captured.clear();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(items)
}

pub struct Torznab {
    meta: EngineMeta,
}

impl Torznab {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        if meta.base_url.as_deref().unwrap_or_default().is_empty() {
            return Err("missing required config base_url".into());
        }
        Ok(Arc::new(Self { meta }))
    }
}

impl Engine for Torznab {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        let mut url = format!("{base}?t=search&q={}", encode(&query.query_text()));
        if let Some(key) = self.meta.api_key.as_deref().filter(|key| !key.is_empty()) {
            url.push_str(&format!("&apikey={key}"));
        }
        let categories = self.meta.opt_str_list("torznab_categories");
        if !categories.is_empty() {
            url.push_str(&format!("&cat={}", categories.join(",")));
        }
        Ok(EngineRequest::get(url))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let show_torrents = self.meta.opt_bool("show_torrent_files").unwrap_or(false);
        let show_magnets = self.meta.opt_bool("show_magnet_links").unwrap_or(true);
        let mut results = Vec::new();
        for item in parse_items(&response.text())? {
            let url = pick(&item, &["guid", "comments"], "http");
            let title = item.get("title");
            if url.is_empty() || title.is_empty() {
                continue;
            }
            let size = if item.get("size").is_empty() {
                item.get("enclosure_length")
            } else {
                item.get("size")
            };
            let filesize = size.parse::<f64>().map(humanize_bytes).unwrap_or_default();
            let leech = if !item.get("leechers").is_empty() {
                item.get("leechers").to_string()
            } else if !item.get("seeders").is_empty() && !item.get("peers").is_empty() {
                (item.get("peers").parse::<i64>().unwrap_or(0)
                    - item.get("seeders").parse::<i64>().unwrap_or(0))
                .to_string()
            } else {
                String::new()
            };
            let published = chrono::DateTime::parse_from_rfc2822(item.get("pubDate"))
                .map(|date| date.to_rfc3339())
                .unwrap_or_default();
            let torrent = if show_torrents {
                pick(&item, &["link", "enclosure_url"], "http")
            } else {
                String::new()
            };
            let keys = ["magneturl", "guid", "enclosure_url", "link"];
            let magnet = if show_magnets {
                pick(&item, &keys, "magnet")
            } else {
                String::new()
            };
            results.push(
                ResultBuilder::new(url, title)
                    .metadata("seed", item.get("seeders"))
                    .metadata("leech", leech)
                    .metadata("filesize", filesize)
                    .metadata("torrentfile", torrent)
                    .metadata("magnetlink", magnet)
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

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("Torznab EZTV", "torznab", "eztv", &["files"]);
        config.base_url = Some("http://localhost:9117/torznab".to_string());
        config.api_key = Some("secret".to_string());
        let categories = vec![
            toml::Value::String("2000".into()),
            toml::Value::String("5000".into()),
        ];
        config
            .extra
            .insert("torznab_categories".into(), toml::Value::Array(categories));
        config
    }

    const FIXTURE: &str = r##"<rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>
      <item><title>Ubuntu 24.04</title><guid>https://example.org/t/1</guid><link>https://example.org/download/1.torrent</link><pubDate>Sun, 21 Feb 2016 21:44:00 +0000</pubDate><size>1572864</size><enclosure url="https://example.org/download/1.torrent" length="1572864"/><torznab:attr name="seeders" value="120"/><torznab:attr name="peers" value="130"/><torznab:attr name="magneturl" value="magnet:?xt=urn:btih:ABC"/></item>
      <item><title>No usable URL</title><guid>not-a-url</guid></item>
    </channel></rss>"##;

    #[test]
    fn builds_and_parses_the_torznab_answer() {
        let engine = Torznab::from_config(&config()).unwrap();
        let query = Query::parse("ubuntu iso", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 1).unwrap();
        assert_eq!(
            request.url,
            "http://localhost:9117/torznab?t=search&q=ubuntu%20iso&apikey=secret&cat=2000,5000"
        );
        let response = EngineResponse::new(200, request.url.clone(), FIXTURE.as_bytes().to_vec());
        let items = engine.parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.org/t/1");
        assert_eq!(result.title, "Ubuntu 24.04");
        assert_eq!(result.metadata["filesize"], "1.50 MB");
        assert_eq!(result.metadata["seed"], "120");
        assert_eq!(result.metadata["leech"], "10");
        assert_eq!(result.metadata["magnetlink"], "magnet:?xt=urn:btih:ABC");
        assert!(
            result
                .published_date
                .as_deref()
                .unwrap()
                .starts_with("2016-02-21")
        );
        let error = r##"<error code="100" description="bad api key"/>"##;
        let response = EngineResponse::new(200, "", error.as_bytes().to_vec());
        assert!(engine.parse(&response, &query).is_err());
    }
}
