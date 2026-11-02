//! Pixiv engine, ported from searx/engines/pixiv.py.
//!
//! An image proxy is required to load the images; the hosts are configured in
//! pixiv_image_proxies (the first one is used).
// engine-id: pixiv

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images"],
    shortcut: "pv",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.pixiv.net/ajax/search/illustrations";
const IMAGE_HOST: &str = "https://i.pximg.net";

pub struct Pixiv {
    meta: EngineMeta,
}

impl Pixiv {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    /// The base URL without its trailing slash.
    fn base(&self) -> String {
        let base = self
            .meta
            .base_url
            .clone()
            .unwrap_or_else(|| BASE_URL.to_string());
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Pixiv {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let text = query.query_text();
        let mut pairs: Vec<(&str, String)> = vec![
            ("word", text.clone()),
            ("order", "date_d".to_string()),
            ("mode", "all".to_string()),
            ("p", pageno.max(1).to_string()),
            ("s_mode", "s_tag_full".to_string()),
            ("type", "illust_and_ugoira".to_string()),
            ("lang", "en".to_string()),
        ];
        if self.meta.opt_bool("remove_ai_images").unwrap_or(false) {
            pairs.push(("ai_type", "1".to_string()));
        }
        // Upstream puts the query in the path as well.
        let url = format!("{}/{}?{}", self.base(), encode(&text), query_string(&pairs));
        Ok(EngineRequest::get(url)
            .header("Referer", "https://www.pixiv.net/")
            .header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(items) = json_path(&json, "body.illust.data").and_then(|value| value.as_array())
        else {
            return Ok(Vec::new());
        };
        let proxies = self.meta.opt_str_list("pixiv_image_proxies");
        let proxy = proxies.first().map(String::as_str);
        let mut results = Vec::new();
        for item in items {
            let image_url = json_str(item, "url");
            if image_url.is_empty() {
                continue;
            }
            let proxy_url = match proxy {
                Some(proxy) => image_url.replace(IMAGE_HOST, proxy),
                None => image_url,
            };
            // Upstream requests the master image through the proxy.
            let full_url = proxy_url
                .replace("/c/250x250_80_a2/", "/")
                .replace("_square1200.jpg", "_master1200.jpg")
                .replace("custom-thumb", "img-master")
                .replace("_custom1200.jpg", "_master1200.jpg");
            let title = json_str(item, "title");
            if title.is_empty() || !full_url.starts_with("http") {
                continue;
            }
            let user_name = json_str(item, "userName");
            let user_id = json_str(item, "userId");
            let author = if user_id.is_empty() {
                user_name
            } else {
                format!("{user_name} (ID: {user_id})")
            };
            results.push(
                ResultBuilder::new(full_url.clone(), title)
                    .content(json_str(item, "alt"))
                    .author(author)
                    .image(full_url)
                    .thumbnail(proxy_url)
                    .template(Template::Images)
                    .metadata("source", "pixiv.net")
                    .build(),
            );
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"{"error": false, "body": {"illust": {"data": [
      {"id": "1", "title": "Sunset",
       "url": "https://i.pximg.net/c/250x250_80_a2/custom-thumb/img/2024/01/01/00/00/00/12345_p0_custom1200.jpg",
       "alt": "A sunset", "userName": "Alice", "userId": "42"},
      {"id": "2", "title": "", "url": "https://i.pximg.net/x.jpg"}
    ]}}}"##;

    const FIXTURE_THUMBNAIL: &str = "https://i.pximg.net/c/250x250_80_a2/custom-thumb/img/2024/01/01/00/00/00/12345_p0_custom1200.jpg";

    fn config() -> EngineConfig {
        EngineConfig::new("pixiv", "pixiv", "pv", &["images"])
    }

    fn response() -> EngineResponse {
        EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec())
    }

    fn query() -> Query {
        Query::parse("sunset", &crate::engine::EngineRegistry::default(), &[])
    }

    #[test]
    fn parses_illust_results() {
        let engine = Pixiv::from_config(&config()).unwrap();
        let items = engine.parse(&response(), &query()).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                let path = "https://i.pximg.net/img-master/img/2024/01/01/00/00/00/12345_p0_master1200.jpg";
                assert_eq!(result.url, path);
                assert_eq!(result.title, "Sunset");
                assert_eq!(result.content, "A sunset");
                assert_eq!(result.author.as_deref(), Some("Alice (ID: 42)"));
                assert_eq!(result.thumbnail.as_deref(), Some(FIXTURE_THUMBNAIL));
                assert_eq!(
                    result.metadata.get("source").map(String::as_str),
                    Some("pixiv.net")
                );
                assert_eq!(result.template, Template::Images);
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn replaces_the_image_host_with_a_proxy() {
        let mut config = config();
        config.extra.insert(
            "pixiv_image_proxies".to_string(),
            toml::Value::Array(vec![toml::Value::String(
                "https://pximg.example.org".to_string(),
            )]),
        );
        let engine = Pixiv::from_config(&config).unwrap();
        let items = engine.parse(&response(), &query()).unwrap();
        match &items[0] {
            EngineItem::Result(result) => {
                let path = "https://pximg.example.org/img-master/img/2024/01/01/00/00/00/12345_p0_master1200.jpg";
                assert_eq!(result.url, path);
                assert!(
                    result
                        .thumbnail
                        .as_deref()
                        .unwrap_or_default()
                        .starts_with("https://pximg.example.org/c/")
                );
            }
            _ => panic!("expected an image result"),
        }
    }

    #[test]
    fn builds_the_search_request() {
        let mut config = config();
        config
            .extra
            .insert("remove_ai_images".to_string(), toml::Value::Boolean(true));
        let engine = Pixiv::from_config(&config).unwrap();
        let query = Query::parse("sunset cat", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine.build_request(&query, 2).unwrap();
        assert!(
            request
                .url
                .starts_with("https://www.pixiv.net/ajax/search/illustrations/sunset%20cat?")
        );
        assert!(request.url.contains("word=sunset%20cat"));
        assert!(request.url.contains("p=2"));
        assert!(request.url.contains("ai_type=1"));
    }
}
