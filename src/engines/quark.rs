//! Quark (Shenma) engine, ported from searx/engines/quark.py.
// engine-id: quark

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "qk",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const GENERAL_URL: &str = "https://quark.sm.cn/s";
const IMAGES_URL: &str = "https://vt.sm.cn/api/pic/list";
const CAPTCHA_RE: &str = "\"action\"\\s*:\\s*\"captcha\"";
const SCRIPT_RE: &str = "(?s)<script\\s+type=\"application/json\"\\s+id=\"s-data-[^\"]+\"\\s+data-used-by=\"hydrate\">(.*?)</script>";
const TITLE_PATHS: &[&str] = &[
    "titleProps.content",
    "title.content",
    "title",
    "data.title",
    "strong.title",
    "company_name",
];
const URL_PATHS: &[&str] = &[
    "sourceProps.dest_url",
    "source.dest_url",
    "source.url",
    "title_url",
    "normal_url",
    "url",
    "data.url",
];
const CONTENT_PATHS: &[&str] = &[
    "summaryProps.content",
    "summary.content",
    "message.replyContent",
    "message.content_text",
    "show_body",
    "desc",
    "data.abstract",
];
const THUMBNAIL_PATHS: &[&str] = &["picListProps.0.src", "image", "img", "message.video_img"];

pub struct Quark {
    meta: EngineMeta,
}

impl Quark {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let category = config.opt_str("quark_category").unwrap_or("general");
        if category != "general" && category != "images" {
            return Err(format!("unsupported quark_category: {category}"));
        }
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }
    fn category(&self) -> &str {
        self.meta.opt_str("quark_category").unwrap_or("general")
    }
}

impl Engine for Quark {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }
    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let per_page = self.meta.opt_i64("results_per_page").unwrap_or(10).max(1);
        if self.category() == "images" {
            let start = (pageno.saturating_sub(1) as i64) * per_page;
            let pairs = vec![
                ("query", query.query_text()),
                ("limit", per_page.to_string()),
                ("start", start.to_string()),
            ];
            return Ok(EngineRequest::get(format!(
                "{IMAGES_URL}?{}",
                query_string(&pairs)
            )));
        }
        let mut pairs = vec![
            ("q", query.query_text()),
            ("layout", "html".to_string()),
            ("page", pageno.to_string()),
        ];
        if let Some(range) = query.time_range {
            let value = match range {
                TimeRange::Day => "4",
                TimeRange::Week => "3",
                TimeRange::Month => "2",
                TimeRange::Year => "1",
            };
            pairs.push(("tl_request", value.to_string()));
        }
        Ok(EngineRequest::get(format!(
            "{GENERAL_URL}?{}",
            query_string(&pairs)
        )))
    }
    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let captcha =
            regex::Regex::new(CAPTCHA_RE).map_err(|error| EngineError::Parse(error.to_string()))?;
        if captcha.is_match(&body) {
            return Err(EngineError::Captcha);
        }
        if self.category() == "images" {
            return parse_images(&body);
        }
        parse_general(&body)
    }
}

fn parse_images(body: &str) -> Result<Vec<EngineItem>, EngineError> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(Vec::new());
    };
    let Some(items) = json_path(&json, "data.hit.imgInfo.item").and_then(|value| value.as_array())
    else {
        return Ok(Vec::new());
    };
    let mut results = Vec::new();
    for item in items {
        let title = strip_tags(&json_str(item, "title"));
        let url = json_str(item, "imgUrl");
        if title.is_empty() || !is_http_url(&url) {
            continue;
        }
        let mut builder = ResultBuilder::new(url, title)
            .template(Template::Images)
            .metadata("source", json_str(item, "site"))
            .metadata("resolution", resolution(item))
            .published_date(item_date(item));
        let image = https(&json_str(item, "bigPicUrl"));
        let thumbnail = https(&json_str(item, "img"));
        if !image.is_empty() {
            builder = builder.image(image);
        }
        if !thumbnail.is_empty() {
            builder = builder.thumbnail(thumbnail);
        }
        results.push(builder.build());
    }
    Ok(results)
}

fn parse_general(body: &str) -> Result<Vec<EngineItem>, EngineError> {
    let pattern =
        regex::Regex::new(SCRIPT_RE).map_err(|error| EngineError::Parse(error.to_string()))?;
    let mut results = Vec::new();
    for captures in pattern.captures_iter(body) {
        let Some(blob) = captures.get(1) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(blob.as_str()) else {
            continue;
        };
        let Some(initial) = json_path(&json, "data.initialData") else {
            continue;
        };
        let sc = json_str(&json, "extraData.sc");
        let key = match sc.as_str() {
            "ai_page" => "list",
            "news_uchq" => "feed",
            "life_show_general_image" => "image",
            "music_new_song" => "hit3",
            _ => "",
        };
        match json_path(initial, key).and_then(|value| value.as_array()) {
            Some(items) => results.extend(items.iter().filter_map(build_item)),
            None => results.extend(build_item(initial)),
        }
    }
    Ok(results)
}

fn build_item(item: &serde_json::Value) -> Option<EngineItem> {
    let title = strip_tags(&pick(item, TITLE_PATHS));
    let url = pick(item, URL_PATHS);
    if title.is_empty() || !is_http_url(&url) {
        return None;
    }
    let mut builder = ResultBuilder::new(url, title)
        .content(strip_tags(&pick(item, CONTENT_PATHS)))
        .published_date(item_date(item));
    let thumbnail = https(&pick(item, THUMBNAIL_PATHS));
    if !thumbnail.is_empty() {
        builder = builder.thumbnail(thumbnail);
    }
    Some(builder.build())
}

fn pick(item: &serde_json::Value, paths: &[&str]) -> String {
    paths
        .iter()
        .map(|path| json_str(item, path))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn item_date(item: &serde_json::Value) -> String {
    let seconds = ["sourceProps.time", "source.time", "publish_time"]
        .into_iter()
        .find_map(|key| json_path(item, key).and_then(serde_json::Value::as_i64));
    match seconds.and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0)) {
        Some(date) => date.to_rfc3339(),
        None => json_str(item, "time"),
    }
}

fn resolution(item: &serde_json::Value) -> String {
    let (width, height) = (json_str(item, "width"), json_str(item, "height"));
    if width.is_empty() || height.is_empty() {
        return String::new();
    }
    format!("{width} x {height}")
}

fn https(url: &str) -> String {
    url.replace("http://", "https://")
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn engine(category: &str) -> Arc<Quark> {
        let mut config = EngineConfig::new("quark", "quark", "qk", &["general"]);
        config.extra.insert(
            "quark_category".to_string(),
            toml::Value::String(category.to_string()),
        );
        Quark::from_config(&config).unwrap()
    }
    fn parse(category: &str, body: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, GENERAL_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        engine(category).parse(&response, &query).unwrap()
    }
    fn first_result(items: &[EngineItem]) -> &crate::results::SearchResult {
        match &items[0] {
            EngineItem::Result(result) => result,
            _ => panic!("expected a result"),
        }
    }
    #[test]
    fn parses_a_general_result_block() {
        let body = r##"<script type="application/json" id="s-data-1" data-used-by="hydrate">{"extraData":{"sc":"ss_doc"},"data":{"initialData":{"titleProps":{"content":"Alpha"},"sourceProps":{"dest_url":"https://example.com/a","time":1700000000},"summaryProps":{"content":"A <b>summary</b>"}}}}</script>"##;
        let items = parse("general", body);
        assert_eq!(items.len(), 1);
        let result = first_result(&items);
        assert_eq!(result.url, "https://example.com/a");
        assert_eq!(result.title, "Alpha");
        assert_eq!(result.content, "A summary");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
    }

    #[test]
    fn parses_the_image_api() {
        let body = r##"{"data":{"hit":{"imgInfo":{"item":[{"imgUrl":"https://example.com/p.jpg","img":"http://img.example.com/t.jpg","bigPicUrl":"http://img.example.com/b.jpg","title":"Cat","site":"example.com","width":800,"height":600,"publish_time":1700000000}]}}}}"##;
        let items = parse("images", body);
        let result = first_result(&items);
        assert_eq!(result.url, "https://example.com/p.jpg");
        assert_eq!(result.template, Template::Images);
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://img.example.com/t.jpg")
        );
        assert_eq!(
            result.img_src.as_deref(),
            Some("https://img.example.com/b.jpg")
        );
        assert_eq!(
            result.metadata.get("resolution").map(String::as_str),
            Some("800 x 600")
        );
    }

    #[test]
    fn detects_the_captcha() {
        let body = r##"{"action":"captcha","url":"https://example.com/x"}"##;
        let response = EngineResponse::new(200, GENERAL_URL, body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        assert!(matches!(
            engine("general").parse(&response, &query),
            Err(EngineError::Captcha)
        ));
    }

    #[test]
    fn forwards_the_time_range() {
        let mut query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        query.time_range = Some(TimeRange::Week);
        let request = engine("general").build_request(&query, 2).unwrap();
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("tl_request=3"));
    }
}
