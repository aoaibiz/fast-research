//! Vuhuv engine, ported from searx/engines/vuhuv.py.
//! General, images and videos share this engine, chosen by vuhuv_category.
// engine-id: vuhuv

use std::sync::Arc;

use scraper::{ElementRef, Html};

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "vu",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://vuhuv.com";

pub struct Vuhuv {
    meta: EngineMeta,
}

impl Vuhuv {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let category = config.opt_str("vuhuv_category").unwrap_or("general");
        if !matches!(category, "general" | "images" | "videos") {
            return Err("vuhuv: invalid vuhuv_category".to_string());
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }

    fn base_url(&self) -> &str {
        self.meta
            .base_url
            .as_deref()
            .unwrap_or(BASE_URL)
            .trim_end_matches('/')
    }

    fn category(&self) -> &str {
        self.meta.opt_str("vuhuv_category").unwrap_or("general")
    }
}

impl Engine for Vuhuv {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let number = match self.category() {
            "images" => "2",
            "videos" => "3",
            _ => "1",
        };
        let pairs: Vec<(&str, String)> = vec![
            ("k", number.to_string()),
            ("p", pageno.max(1).to_string()),
            ("q", query.query_text()),
            ("d", "1".to_string()),
            ("dh", "1".to_string()),
        ];
        let base = self.base_url();
        Ok(
            EngineRequest::get(format!("{base}/veri2/?{}", query_string(&pairs)))
                .header("Referer", &format!("{base}/")),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        Ok(match self.category() {
            "images" => image_results(&html, self.base_url()),
            "videos" => video_results(&html, self.base_url()),
            _ => general_results(&html, self.base_url()),
        })
    }
}

fn general_results(html: &Html, base: &str) -> Vec<EngineItem> {
    let Ok(rows) = select_elements(html, "div[class*='sonuc'] > div") else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for row in rows {
        let Some(url) = result_url(row, base) else {
            continue;
        };
        let title = text_of(row, "a span").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let content = text_of(row, "ins").unwrap_or_default();
        results.push(ResultBuilder::new(url, title).content(content).build());
    }
    results
}

fn image_results(html: &Html, base: &str) -> Vec<EngineItem> {
    let Ok(rows) = select_elements(html, "div[class*='item gorsel']") else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for row in rows {
        let Some(url) = result_url(row, base) else {
            continue;
        };
        let title = attr_of(row, "a", "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let resolution = text_of(row, "div[class*='olculeri']").unwrap_or_default();
        let mut builder = ResultBuilder::new(url, title)
            .template(Template::Images)
            .metadata("resolution", resolution);
        if let Some(src) = attr(row, "data-resimurl").filter(|src| is_http_url(src)) {
            builder = builder.image(src);
        }
        let thumbnail = format!("https:{}", attr(row, "data-kgorsel").unwrap_or_default());
        if is_http_url(&thumbnail) {
            builder = builder.thumbnail(thumbnail);
        }
        results.push(builder.build());
    }
    results
}

fn video_results(html: &Html, base: &str) -> Vec<EngineItem> {
    let Ok(rows) = select_elements(html, "div[class*='item video']") else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for row in rows {
        let Some(url) = result_url(row, base) else {
            continue;
        };
        let title = attr_of(row, "a", "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let content = text_of(row, "div[class*='abaslik']").unwrap_or_default();
        let mut builder = ResultBuilder::new(url, title)
            .content(content)
            .template(Template::Videos);
        if let Some(thumbnail) = attr(row, "data-kgorsel").filter(|src| is_http_url(src)) {
            builder = builder.thumbnail(thumbnail);
        }
        if let Some(iframe) = attr(row, "data-embedurl").filter(|src| is_http_url(src)) {
            builder = builder.metadata("iframe_src", iframe);
        }
        results.push(builder.build());
    }
    results
}

fn result_url(row: ElementRef<'_>, base: &str) -> Option<String> {
    absolute_url(&attr_of(row, "a", "href")?, base).filter(|url| is_http_url(url))
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERAL: &str = r##"<html><body><div class="sonuc"><div><a href="https://example.com/rust"><span>Rust</span></a><ins>A fast language.</ins></div></div></body></html>"##;
    const IMAGES: &str = r##"<html><body><div class="item gorsel" data-kgorsel="//cdn.example/cat.jpg" data-resimurl="https://cdn.example/cat-full.jpg"><a href="https://example.com/cat" title="Cat"><div class="olculeri">800x600</div></a></div></body></html>"##;
    const CAT: &str = "https://cdn.example/cat.jpg";

    fn engine(category: &str) -> Arc<Vuhuv> {
        let mut config = EngineConfig::new("vuhuv", "vuhuv", "vu", &["general"]);
        let value = toml::Value::String(category.into());
        config.extra.insert("vuhuv_category".into(), value);
        Vuhuv::from_config(&config).unwrap()
    }

    fn items(category: &str, body: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        engine(category).parse(&response, &query).unwrap()
    }

    #[test]
    fn parses_categories_and_builds_the_request() {
        assert_eq!(items("general", GENERAL).len(), 1);
        let images = items("images", IMAGES);
        assert!(
            matches!(&images[0], EngineItem::Result(image) if image.thumbnail.as_deref() == Some(CAT))
        );
        let mut broken = EngineConfig::new("vuhuv", "vuhuv", "vu", &["general"]);
        let value = toml::Value::String("news".into());
        broken.extra.insert("vuhuv_category".into(), value);
        assert!(Vuhuv::from_config(&broken).is_err());
        let query = Query::parse("rust lang", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine("images").build_request(&query, 2).unwrap();
        assert!(request.url.contains("k=2&p=2&q=rust%20lang"));
    }
}
