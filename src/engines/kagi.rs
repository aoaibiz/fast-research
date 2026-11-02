//! Kagi engine, ported from searx/engines/kagi.py.
//!
//! Requires an API key. The workflow (search, images, news or videos) comes
//! from the kagi_categ setting, like the four upstream entries.
// engine-id: kagi

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
    shortcut: "kg",
    paging: true,
    time_range_support: true,
    safesearch: true,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const BASE_URL: &str = "https://kagi.com";
const WORKFLOWS: [&str; 4] = ["search", "images", "news", "videos"];

pub struct Kagi {
    meta: EngineMeta,
}

impl Kagi {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        if meta
            .api_key
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            return Err("kagi requires an api key".to_string());
        }
        let workflow = meta.opt_str("kagi_categ").unwrap_or("search");
        if !WORKFLOWS.contains(&workflow) {
            return Err(format!("kagi: unsupported kagi_categ {workflow:?}"));
        }
        Ok(Arc::new(Self { meta }))
    }

    fn base_url(&self) -> String {
        let base = self.meta.base_url.as_deref().unwrap_or(BASE_URL);
        base.trim_end_matches('/').to_string()
    }
}

impl Engine for Kagi {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        if pageno > 10 {
            return Err(EngineError::Message(
                "kagi serves at most 10 pages".to_string(),
            ));
        }
        let api_key = self.meta.api_key.as_deref().unwrap_or_default();
        let mut filters = serde_json::json!({"region": region(&query.locale)});
        if let Some(days) = query.time_range.map(|range| match range {
            TimeRange::Day => 1,
            TimeRange::Week => 7,
            TimeRange::Month => 30,
            TimeRange::Year => 365,
        }) {
            let oldest = chrono::Utc::now() - chrono::Duration::days(days);
            filters["after"] = oldest.format("%Y-%m-%d").to_string().into();
        }
        let body = serde_json::json!({
            "query": query.query_text(),
            "page": pageno,
            "workflow": self.meta.opt_str("kagi_categ").unwrap_or("search"),
            "safe_search": query.safesearch != 0,
            "filters": filters,
        });
        Ok(
            EngineRequest::post(format!("{}/api/v1/search", self.base_url()))
                .header("Authorization", &format!("Bearer {api_key}"))
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .json_body(body),
        )
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(data) = json_path(&json, "data") else {
            return Ok(Vec::new());
        };
        let workflow = self.meta.opt_str("kagi_categ").unwrap_or("search");
        let key = match workflow {
            "images" => "image",
            "videos" => "video",
            other => other,
        };
        let Some(entries) = json_path(data, key).and_then(serde_json::Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut items: Vec<EngineItem> = entries
            .iter()
            .filter_map(|entry| parse_entry(entry, workflow))
            .collect();
        for suggestion in json_path(data, "related_search")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let label = json_str(suggestion, "title");
            if !label.is_empty() {
                items.push(EngineItem::Suggestion(label));
            }
        }
        Ok(items)
    }
}

fn parse_entry(entry: &serde_json::Value, workflow: &str) -> Option<EngineItem> {
    let url = json_str(entry, "url");
    let title = strip_tags(&json_str(entry, "title"));
    if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let image = json_str(entry, "image.url");
    if workflow == "images" {
        let mut builder = ResultBuilder::new(url, title).template(Template::Images);
        if !image.is_empty() {
            builder = builder.image(image);
        }
        let preview = json_str(entry, "props.thumbnail.url");
        if !preview.is_empty() {
            builder = builder.thumbnail(preview);
        }
        let width = json_str(entry, "image.width");
        let height = json_str(entry, "image.height");
        if !width.is_empty() && !height.is_empty() {
            builder = builder.metadata("resolution", format!("{width}x{height}"));
        }
        return Some(builder.build());
    }
    let mut builder = ResultBuilder::new(url, title)
        .content(strip_tags(&json_str(entry, "snippet")))
        .published_date(date(&json_str(entry, "time")).unwrap_or_default());
    if !image.is_empty() {
        builder = builder.thumbnail(image);
    }
    if workflow == "videos" {
        builder = builder
            .template(Template::Videos)
            .author(json_str(entry, "props.creator_name"));
    }
    Some(builder.build())
}

fn region(locale: &str) -> String {
    locale.trim().rsplit_once('-').map_or_else(
        || "no_region".to_string(),
        |(_, region)| region.to_lowercase(),
    )
}

/// Upstream parses the timestamp with fromisoformat.
fn date(value: &str) -> Option<String> {
    let value = value.trim();
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.to_rfc3339())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|parsed| parsed.format("%Y-%m-%dT%H:%M:%S").to_string())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .map(|parsed| parsed.format("%Y-%m-%dT00:00:00").to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(categ: &str) -> Arc<Kagi> {
        let mut config = EngineConfig::new("kagi", "kagi", "kg", &["general"]);
        config.api_key = Some("kagi_test".to_string());
        config
            .extra
            .insert("kagi_categ".into(), toml::Value::String(categ.into()));
        Kagi::from_config(&config).unwrap()
    }

    fn query(raw: &str) -> Query {
        Query::parse(raw, &crate::engine::EngineRegistry::default(), &[])
    }

    fn response(body: &str) -> EngineResponse {
        EngineResponse::new(200, BASE_URL, body.as_bytes().to_vec())
    }

    #[test]
    fn parses_search_results_and_suggestions() {
        let fixture = r##"{"data":{"search":[{"url":"https://example.com/a","title":"A <b>title</b>","snippet":"A <i>snippet</i>","time":"2024-02-03T10:20:30Z","image":{"url":"https://img.example.com/a.jpg"}},{"url":"ftp://example.com/b","title":"Bad"},{"url":"https://example.com/c","title":""}],"related_search":[{"title":"related thing"},{"title":""}]}}"##;
        let items = engine("search")
            .parse(&response(fixture), &query("rust"))
            .unwrap();
        assert_eq!(items.len(), 2);
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.url, "https://example.com/a");
        assert_eq!(result.title, "A title");
        assert_eq!(result.content, "A snippet");
        assert_eq!(
            result.published_date.as_deref(),
            Some("2024-02-03T10:20:30+00:00")
        );
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://img.example.com/a.jpg")
        );
        assert!(matches!(&items[1], EngineItem::Suggestion(value) if value == "related thing"));
    }

    #[test]
    fn parses_image_results_and_builds_the_request() {
        let fixture = r##"{"data":{"image":[{"url":"https://example.com/i","title":"Pic","image":{"url":"https://img.example.com/i.jpg","width":800,"height":600},"props":{"thumbnail":{"url":"https://img.example.com/t.jpg"}}}]}}"##;
        let items = engine("images")
            .parse(&response(fixture), &query("cat"))
            .unwrap();
        let EngineItem::Result(result) = &items[0] else {
            panic!("expected a result")
        };
        assert_eq!(result.template, Template::Images);
        assert_eq!(
            result.img_src.as_deref(),
            Some("https://img.example.com/i.jpg")
        );
        assert_eq!(
            result.metadata.get("resolution").map(String::as_str),
            Some("800x600")
        );
        assert_eq!(
            result.thumbnail.as_deref(),
            Some("https://img.example.com/t.jpg")
        );
        let mut request_query = query("rust");
        request_query.locale = "ja-JP".to_string();
        request_query.time_range = Some(TimeRange::Week);
        request_query.safesearch = 2;
        let request = engine("search").build_request(&request_query, 2).unwrap();
        assert_eq!(request.method, crate::engine::Method::Post);
        assert_eq!(request.url, "https://kagi.com/api/v1/search");
        assert_eq!(
            request.headers["authorization"].to_str().unwrap(),
            "Bearer kagi_test"
        );
        let body = request.json.expect("a JSON body");
        assert_eq!(body["page"], 2);
        assert_eq!(body["safe_search"], true);
        assert_eq!(body["filters"]["region"], "jp");
        assert!(
            chrono::NaiveDate::parse_from_str(
                body["filters"]["after"].as_str().unwrap(),
                "%Y-%m-%d"
            )
            .is_ok()
        );
        assert!(engine("search").build_request(&request_query, 11).is_err());
        assert!(Kagi::from_config(&EngineConfig::new("kagi", "kagi", "kg", &["general"])).is_err());
        assert!(
            engine("search")
                .parse(&response("{}"), &query("x"))
                .unwrap()
                .is_empty()
        );
    }
}
