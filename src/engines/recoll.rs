//! Recoll engine, ported from searx/engines/recoll.py.
//!
//! Recoll runs its own web UI; the engine queries its JSON endpoint.

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

// engine-id: recoll

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: true,
    time_range_support: true,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 6.0,
};

/// A Recoll web UI instance.
pub struct Recoll {
    meta: EngineMeta,
    mount_prefix: String,
    dl_prefix: String,
    search_dir: String,
}

impl Recoll {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        if meta.base_url.is_none() || meta.base_url.as_deref() == Some("") {
            return Err("recoll requires the base_url option".into());
        }
        Ok(Arc::new(Self {
            mount_prefix: meta.opt_str("mount_prefix").unwrap_or("").to_string(),
            dl_prefix: meta.opt_str("dl_prefix").unwrap_or("").to_string(),
            search_dir: meta.opt_str("search_dir").unwrap_or("").to_string(),
            meta,
        }))
    }
}

impl Engine for Recoll {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, query: &Query, pageno: u32) -> Result<EngineRequest, EngineError> {
        let base = self
            .meta
            .base_url
            .as_deref()
            .ok_or_else(|| EngineError::Message("recoll requires base_url".into()))?
            .trim_end_matches('/');
        let after = crate::query::TimeRange::seconds(query.time_range)
            .map(|seconds| chrono::Utc::now() - chrono::Duration::seconds(seconds));
        let mut pairs = vec![
            ("query", query.query_text().to_string()),
            ("page", pageno.to_string()),
            ("highlight", "0".to_string()),
        ];
        if let Some(after) = after {
            pairs.push(("after", after.format("%Y-%m-%d").to_string()));
        }
        if !self.search_dir.is_empty() {
            pairs.push(("dir", self.search_dir.clone()));
        }
        Ok(EngineRequest::get(format!(
            "{base}/json?{}",
            query_string(&pairs)
        )))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let json = response.json()?;
        let Some(entries) = json_path(&json, "results").and_then(serde_json::Value::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for entry in entries {
            let url = json_str(entry, "url")
                .replace(&format!("file://{}", self.mount_prefix), &self.dl_prefix);
            let title = json_str(entry, "label");
            if url.is_empty() || title.is_empty() {
                continue;
            }
            let mime = json_str(entry, "mtype");
            let (mime_type, _) = mime.split_once('/').unwrap_or((mime.as_str(), ""));
            let mut builder = ResultBuilder::new(url.clone(), title)
                .content(strip_tags(&json_str(entry, "snippet")))
                .author(json_str(entry, "author"))
                .metadata("size", json_str(entry, "size"))
                .metadata("filename", json_str(entry, "filename"));
            if mime_type == "audio" || mime_type == "video" {
                builder = builder.metadata("iframe_src", url.clone());
            }
            if mime_type == "image"
                && matches!(
                    mime.split('/').nth(1).unwrap_or(""),
                    "bmp" | "gif" | "jpeg" | "png"
                )
            {
                builder = builder.thumbnail(url);
            }
            results.push(builder.build());
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Recoll> {
        let mut config = EngineConfig::new("recoll", "recoll", "", &["general"]);
        config.base_url = Some("http://localhost:8080/".to_string());
        Recoll::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_a_base_url() {
        let bare = EngineConfig::new("recoll", "recoll", "", &["general"]);
        assert!(Recoll::from_config(&bare).is_err());
    }

    #[test]
    fn queries_the_json_endpoint() {
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let request = engine().build_request(&query, 2).unwrap();
        assert!(request.url.starts_with("http://localhost:8080/json?"));
        assert!(request.url.contains("query=rust"));
        assert!(request.url.contains("page=2"));
    }

    #[test]
    fn parses_the_results() {
        let body = r##"{"results":[
            {"url":"file:///docs/a.pdf","label":"Doc A","snippet":"about rust",
             "mtype":"application/pdf","size":"1 MB","filename":"a.pdf"}
        ]}"##;
        let response = EngineResponse::new(200, "http://localhost:8080", body.as_bytes().to_vec());
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine().parse(&response, &query).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Doc A");
                // upstream applies the (empty) mount/dl prefix mapping
                assert_eq!(result.url, "/docs/a.pdf");
            }
            _ => panic!("expected a result"),
        }
    }
}
