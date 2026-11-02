//! FindFiles.net engine, ported from `searx/engines/findfiles.py` (file search).
//!
//! Upstream exposes a `findfiles_categ` setting with the values `all`,
//! `document`, `text`, `image`, `audio` and `video`; the main variant ported
//! here is `all`, and the setting is read from the `search_type` config key.
// engine-id: findfiles

use std::path::Path;
use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["files"],
    shortcut: "ff",
    paging: true,
    time_range_support: false,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://findfiles.net";

/// FindFiles.net, a German file search engine.
pub struct FindFiles {
    meta: EngineMeta,
    categ: String,
}

impl FindFiles {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let categ = config.opt_str("search_type").unwrap_or("all").to_string();
        if !matches!(
            categ.as_str(),
            "all" | "document" | "text" | "image" | "audio" | "video"
        ) {
            return Err(format!("findfiles: invalid search_type {categ:?}"));
        }
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
            categ,
        }))
    }
}

impl Engine for FindFiles {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let contentguard = match query.safesearch {
            0 => "contentguard.off",
            1 => "contentguard.moderate",
            _ => "contentguard.strict",
        };
        let pairs = vec![
            ("query", query.query_text()),
            ("contentguard", contentguard.to_string()),
            ("page", pageno.max(1).to_string()),
        ];
        Ok(EngineRequest::get(format!(
            "{BASE_URL}/en/serp/{}/?{}",
            self.categ,
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let html = parse_html(&response.text());
        let mut results = Vec::new();
        if self.categ == "image" {
            let Ok(containers) = select_elements(&html, "div.image-mosaic > div.image-item") else {
                return Ok(results);
            };
            for container in containers {
                let Some(href) = attr_of(container, "figcaption a", "href") else {
                    continue;
                };
                let title = text_of(container, "figcaption a").unwrap_or_default();
                let thumbnail = attr_of(container, "img", "src");
                let url = absolute_url(&href, BASE_URL).unwrap_or_default();
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                let mut builder = ResultBuilder::new(url, title).template(Template::Images);
                if let Some(thumbnail) = thumbnail {
                    builder = builder.thumbnail(thumbnail);
                }
                results.push(builder.build());
            }
        } else if self.categ == "video" {
            let Ok(containers) = select_elements(&html, "div.video-mosaic > div.video-item") else {
                return Ok(results);
            };
            for container in containers {
                let Some(video_src) = attr_of(container, "video", "src") else {
                    continue;
                };
                let title = text_of(container, "div.caption > span").unwrap_or_default();
                let url = absolute_url(&video_src, BASE_URL).unwrap_or_default();
                if title.is_empty() || !is_http_url(&url) {
                    continue;
                }
                results.push(
                    ResultBuilder::new(url, title)
                        .template(Template::Videos)
                        .build(),
                );
            }
        } else {
            let Ok(entries) = select_elements(&html, "ol li.result-item > article") else {
                return Ok(results);
            };
            for entry in entries {
                let Some(href) = attr_of(entry, "h3 a", "href") else {
                    continue;
                };
                let url = absolute_url(&href, BASE_URL).unwrap_or_default();
                if !is_http_url(&url) {
                    continue;
                }
                // The result title is the file name of the linked file.
                let filename = Path::new(&url)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let title = text_of(entry, "h3").unwrap_or_else(|| filename.clone());
                if title.is_empty() {
                    continue;
                }
                let content = children(entry, "div > span")
                    .iter()
                    .map(|span| text(*span))
                    .collect::<Vec<_>>()
                    .join(" ");
                results.push(
                    ResultBuilder::new(url, title)
                        .content(content)
                        .metadata("filename", filename)
                        .metadata("size", text_of(entry, "span[id]").unwrap_or_default())
                        .build(),
                );
            }
        }
        Ok(results)
    }
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<html><body><ol>
      <li class="result-item"><article>
        <h3><a href="https://files.example.com/pub/report.pdf">report.pdf</a></h3>
        <div><span>2.4 MB</span><span>PDF</span></div>
      </article></li>
    </ol></body></html>"#;

    fn engine() -> Arc<FindFiles> {
        FindFiles::from_config(&EngineConfig::new(
            "findfiles",
            "findfiles",
            "ff",
            &["files"],
        ))
        .unwrap()
    }

    #[test]
    fn parses_file_results() {
        let response = EngineResponse::new(
            200,
            "https://findfiles.net/en/serp/all/",
            FIXTURE.as_bytes().to_vec(),
        );
        let query = Query::parse("report", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "report.pdf");
                assert_eq!(result.url, "https://files.example.com/pub/report.pdf");
            }
            _ => panic!("expected a result"),
        }
    }

    #[test]
    fn builds_the_paged_url() {
        let query = Query::parse("report", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 3).unwrap();
        assert!(
            request
                .url
                .starts_with("https://findfiles.net/en/serp/all/?")
        );
        assert!(request.url.contains("page=3"));
        assert!(request.url.contains("contentguard.off"));
    }
}
