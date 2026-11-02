//! YouTube engine without an API key, ported from
//! `searx/engines/youtube_noapi.py` (the HTML result page; the upstream
//! continuation-token request is not ported).

use std::sync::Arc;

use scraper::ElementRef;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::{Query, TimeRange};
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["videos"],
    shortcut: "yt",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://www.youtube.com/results";
const YOUTUBE_BASE: &str = "https://www.youtube.com";

/// The sp filter values upstream uses for the time range.
fn time_range_code(range: TimeRange) -> &'static str {
    match range {
        TimeRange::Day => "Ag",
        TimeRange::Week => "Aw",
        TimeRange::Month => "BA",
        TimeRange::Year => "BQ",
    }
}

/// YouTube's HTML search page.
pub struct YoutubeNoapi {
    meta: EngineMeta,
}

impl YoutubeNoapi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for YoutubeNoapi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let pairs = vec![
            ("search_query", query.query_text()),
            ("page", pageno.max(1).to_string()),
        ];
        let mut url = format!("{BASE_URL}?{}", query_string(&pairs));
        if let Some(range) = query.time_range {
            url.push_str(&format!("&sp=EgII{}%253D%253D", time_range_code(range)));
        }
        Ok(EngineRequest::get(url)
            .cookie("CONSENT", "YES+")
            .header("Accept", "text/html,application/xhtml+xml"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = response.html()?;
        let mut results = Vec::new();
        for element in select_elements(&html, "ytd-video-renderer")? {
            let Some(link) = child(element, "a#video-title") else {
                continue;
            };
            let Some(href) = attr(link, "href") else {
                continue;
            };
            let Some(url) = absolute_url(&href, YOUTUBE_BASE) else {
                continue;
            };
            if !is_http_url(&url) {
                continue;
            }
            let title = text(link);
            if title.is_empty() {
                continue;
            }
            let metadata = children(element, "span.ytd-video-meta-block")
                .iter()
                .map(|span| text(*span))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" - ");
            let mut builder = ResultBuilder::new(url, title)
                .content(metadata)
                .template(Template::Videos)
                .author(text_of(element, "ytd-channel-name a").unwrap_or_default())
                .metadata(
                    "length",
                    text_of(element, "span.ytd-thumbnail-overlay-time-status-renderer")
                        .unwrap_or_default(),
                );
            if let Some(thumbnail) = thumbnail(element) {
                builder = builder.thumbnail(thumbnail);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// The first image of the thumbnail container, including the lazy variants.
fn thumbnail(element: ElementRef<'_>) -> Option<String> {
    let image = child(element, "img")?;
    attr(image, "src")
        .or_else(|| attr(image, "data-src"))
        .or_else(|| attr(image, "data-thumbnail-url"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
    <html><body>
    <ytd-item-section-renderer>
      <ytd-video-renderer class="style-scope ytd-item-section-renderer">
        <div id="thumbnail">
          <a id="thumbnail" href="/watch?v=dQw4w9WgXcQ">
            <img id="img" src="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg">
            <span class="ytd-thumbnail-overlay-time-status-renderer">3:33</span>
          </a>
        </div>
        <div id="dismissible">
          <ytd-channel-name id="channel-name">
            <div id="container"><div id="text-container"><a href="/@RickAstley">Rick Astley</a></div></div>
          </ytd-channel-name>
          <a id="video-title" class="yt-simple-endpoint" href="/watch?v=dQw4w9WgXcQ">
            Rick Astley - Never Gonna Give You Up (Official Video)
          </a>
          <div id="metadata">
            <span class="ytd-video-meta-block">14M views</span>
            <span class="ytd-video-meta-block">16 years ago</span>
          </div>
        </div>
      </ytd-video-renderer>
    </ytd-item-section-renderer>
    </body></html>"#;

    fn engine() -> Arc<YoutubeNoapi> {
        YoutubeNoapi::from_config(&EngineConfig::new(
            "youtube",
            "youtube_noapi",
            "yt",
            &["videos"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_video_renderers() {
        let response = EngineResponse::new(200, BASE_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse(
            "rick astley",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(
                    result.title,
                    "Rick Astley - Never Gonna Give You Up (Official Video)"
                );
                assert_eq!(result.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
                assert_eq!(result.template, Template::Videos);
                assert_eq!(result.author.as_deref(), Some("Rick Astley"));
                assert_eq!(result.content, "14M views - 16 years ago");
                assert_eq!(
                    result.metadata.get("length").map(String::as_str),
                    Some("3:33")
                );
                assert_eq!(
                    result.thumbnail.as_deref(),
                    Some("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg")
                );
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn forwards_paging_and_time_range() {
        let mut query = Query::parse(
            "rick astley",
            &crate::engine::EngineRegistry::default(),
            &[],
        );
        query.time_range = Some(TimeRange::Week);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.contains("page=2"));
        assert!(request.url.contains("sp=EgIIAw%253D%253D"));
        assert!(
            request
                .cookies
                .contains(&("CONSENT".to_string(), "YES+".to_string()))
        );
    }
}
