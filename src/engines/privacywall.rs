//! Privacywall engine, ported from searx/engines/privacywall.py.
// engine-id: privacywall

use std::sync::Arc;

use scraper::ElementRef;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "pw",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.privacywall.org";
const VIDEO_PAGES: [&str; 9] = [
    "CAoQAA", "CBQQAA", "CB4QAA", "CCgQAA", "CDIQAA", "CDwQAA", "CEYQAA", "CFAQAA", "CFoQAA",
];

pub struct Privacywall {
    meta: EngineMeta,
}

impl Privacywall {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        Ok(Arc::new(Self { meta }))
    }

    fn base_url(&self) -> &str {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/')
    }

    fn category(&self) -> &str {
        let key = "privacywall_category";
        self.meta.opt_str(key).unwrap_or("general")
    }
}

impl Engine for Privacywall {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let category = self.category();
        if !matches!(category, "general" | "images" | "videos") {
            return Err(EngineError::Message("privacywall: bad category".into()));
        }
        if pageno > 10 {
            return Err(EngineError::Message("privacywall: only 10 pages".into()));
        }
        if category == "general" && pageno > 1 {
            return Err(EngineError::Message("privacywall: no pagination".into()));
        }
        let base = self.base_url();
        let safesearch = if query.safesearch == 0 { "off" } else { "on" };
        let mut pairs: Vec<(&str, String)> =
            vec![("q", query.query_text()), ("safesearch", safesearch.into())];
        let region = locale_region(&query.locale);
        if region != "all" {
            pairs.push(("cc", region.to_uppercase()));
        }
        if let Some(range) = query.time_range {
            pairs.push(("time", range.as_str().to_string()));
        }
        if pageno > 1 {
            let page = match category {
                "images" => pageno.to_string(),
                _ => video_page(pageno),
            };
            pairs.push(("page", page));
        }
        let path = match category {
            "general" => "search/secure",
            other => other,
        };
        let url = format!("{base}/{path}/?{}", query_string(&pairs));
        Ok(EngineRequest::get(url).header("Referer", &format!("{base}/")))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let base = self.base_url();
        let category = self.category();
        let pattern = match category {
            "images" => "div#container div[class*='imgcontainer']",
            "videos" => "div[class*='video-container'] > div[class*='video-card']",
            _ => "div#pw-results-main div[class*='result-card']",
        };
        let mut results = Vec::new();
        for element in select_elements(&html, pattern)? {
            let item = match category {
                "images" => image_result(element, base),
                "videos" => video_result(element, base),
                _ => general_result(element, base),
            };
            results.extend(item);
        }
        Ok(results)
    }
}

fn general_result(element: ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let href = attr_of(element, "a[class*='result-url-anchor']", "href")?;
    let url = http_url(&href, base)?;
    let title = text_of(element, "div[class*='result_title']")?;
    let content = text_of(element, "div[class*='result-description']").unwrap_or_default();
    Some(ResultBuilder::new(url, title).content(content).build())
}

fn image_result(element: ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let url = http_url(&attr_of(element, "a", "href")?, base)?;
    let title = attr_of(element, "a", "alt")?;
    let source = text_of(element, "div[class*='image-source-badge']").unwrap_or_default();
    let thumb = extract_thumbnail_url(&attr_of(element, "img", "src").unwrap_or_default());
    let mut builder = ResultBuilder::new(url, title.clone()).content(title);
    builder = builder.template(Template::Images);
    builder = builder.metadata("source", source);
    if thumb.starts_with("http") {
        builder = builder.image(thumb.clone());
        builder = builder.thumbnail(thumb);
    }
    Some(builder.build())
}

fn video_result(element: ElementRef<'_>, base: &str) -> Option<EngineItem> {
    let url = http_url(&attr_of(element, "a", "href")?, base)?;
    let title = text_of(element, "h2[class*='video-card-title']")?;
    let content = text_of(element, "p").unwrap_or_default();
    let style = attr_of(element, "div[class*='video-img']", "style").unwrap_or_default();
    let mut thumb = String::new();
    if let Some(index) = style.find(":url(") {
        let raw = style[index + 5..].split(')').next().unwrap_or_default();
        thumb = extract_thumbnail_url(raw);
    }
    let mut builder = ResultBuilder::new(url, title).content(content);
    builder = builder.template(Template::Videos);
    if thumb.starts_with("http") {
        builder = builder.thumbnail(thumb);
    }
    Some(builder.build())
}

fn http_url(raw: &str, base: &str) -> Option<String> {
    absolute_url(raw, base).filter(|url| url.starts_with("http"))
}

fn extract_thumbnail_url(url: &str) -> String {
    if let Some(index) = url.find("?id=") {
        let raw = url[index + 4..].replace('+', " ");
        let decoded = percent_encoding::percent_decode_str(&raw);
        decoded.decode_utf8_lossy().to_string()
    } else {
        String::new()
    }
}

fn video_page(pageno: u32) -> String {
    let token = VIDEO_PAGES.get(pageno.wrapping_sub(2) as usize).copied();
    token.unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    const GENERAL: &str = r#"<div id="pw-results-main"><div class="result-card"><a class="result-url-anchor" href="https://example.com/page">x</a><div class="result_title">Example title</div><div class="result-description">An example description.</div></div></div>"#;
    const IMAGES: &str = r#"<div id="container"><div class="imgcontainer"><a href="https://example.com/photo" alt="A photo"><img src="/videos/video.php?id=https%3A%2F%2Fimg.example%2Fp.jpg"></a><div class="image-source-badge">example.com</div></div></div>"#;
    const VIDEOS: &str = r#"<div class="video-container"><div class="video-card"><a href="https://videos.example/watch/1"><h2 class="video-card-title">A video</h2></a><p>Video text.</p><div class="video-img" style="background-image:url(/videos/video.php?id=https%3A%2F%2Fvideos.example%2Ft.jpg)"></div></div></div>"#;

    fn engine(category: &str) -> Arc<Privacywall> {
        let mut config = EngineConfig::new("privacywall", "privacywall", "pw", &["general"]);
        let key = "privacywall_category";
        config.extra.insert(key.into(), category.into());
        Privacywall::from_config(&config).unwrap()
    }

    fn first(category: &str, fixture: &str) -> crate::results::SearchResult {
        let response = EngineResponse::new(200, BASE_URL, fixture.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let mut items = engine(category).parse(&response, &query).unwrap();
        let EngineItem::Result(result) = items.remove(0) else {
            panic!()
        };
        result
    }
    #[test]
    fn parses_and_builds_requests() {
        let general = first("general", GENERAL);
        assert_eq!(general.url, "https://example.com/page");
        let images = first("images", IMAGES);
        assert_eq!(images.template, Template::Images);
        assert!(images.thumbnail.unwrap_or_default().ends_with("/p.jpg"));
        let videos = first("videos", VIDEOS);
        assert_eq!(videos.template, Template::Videos);
        assert!(videos.thumbnail.unwrap_or_default().ends_with("/t.jpg"));
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine("images").build_request(&query, 2).unwrap();
        assert!(request.url.contains("page=2"));
        assert!(engine("general").build_request(&query, 2).is_err());
    }
}
