//! MongoDB engine, ported from searx/engines/mongodb.py.
//!
//! An offline engine: it searches one field of a collection and returns the
//! matching documents as key/value results.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::util::ResultBuilder;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: mongodb

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: true,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 5.0,
};

/// A MongoDB collection.
pub struct Mongodb {
    meta: EngineMeta,
    host: String,
    port: u16,
    username: String,
    password: String,
    database: String,
    collection: String,
    key: String,
    exact_match_only: bool,
    results_per_page: u32,
}

impl Mongodb {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let database = meta.opt_str("database").unwrap_or("").to_string();
        let collection = meta.opt_str("collection").unwrap_or("").to_string();
        let key = meta.opt_str("key").unwrap_or("").to_string();
        if database.is_empty() || collection.is_empty() || key.is_empty() {
            return Err("mongodb requires the database, collection and key options".into());
        }
        Ok(Arc::new(Self {
            host: meta.opt_str("host").unwrap_or("127.0.0.1").to_string(),
            port: meta
                .opt_i64("port")
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(27017),
            username: meta.opt_str("username").unwrap_or("").to_string(),
            password: meta.opt_str("password").unwrap_or("").to_string(),
            database,
            collection,
            key,
            exact_match_only: meta.opt_bool("exact_match_only").unwrap_or(true),
            results_per_page: meta
                .opt_i64("results_per_page")
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(20),
            meta,
        }))
    }

    fn connection_uri(&self) -> String {
        if self.username.is_empty() {
            format!("mongodb://{}:{}", self.host, self.port)
        } else {
            format!(
                "mongodb://{}:{}@{}:{}",
                self.username, self.password, self.host, self.port
            )
        }
    }
}

impl Engine for Mongodb {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message("mongodb is an offline engine".into()))
    }

    fn parse(
        &self,
        _response: &EngineResponse,
        _query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        Ok(Vec::new())
    }

    fn is_offline(&self) -> bool {
        true
    }

    fn search_offline(&self, query: &Query, pageno: u32) -> Result<Vec<EngineItem>, EngineError> {
        use bson::doc;
        use futures::TryStreamExt;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                EngineError::Message(format!("mongodb: cannot start the runtime: {error}"))
            })?;
        let timeout = self.meta.timeout.max(Duration::from_millis(200));
        let query_text = query.query_text().to_string();
        let skip = u64::from(pageno.saturating_sub(1)) * u64::from(self.results_per_page);

        runtime.block_on(async move {
            let client = mongodb::Client::with_uri_str(self.connection_uri())
                .await
                .map_err(|error| {
                    EngineError::Message(format!("mongodb: cannot connect: {error}"))
                })?;
            let collection = client
                .database(&self.database)
                .collection::<bson::Document>(&self.collection);
            let filter = if self.exact_match_only {
                doc! { &self.key: { "$eq": &query_text } }
            } else {
                let pattern = regex_escape(&query_text);
                doc! { &self.key: { "$regex": pattern, "$options": "i" } }
            };
            let future = async {
                let mut cursor = collection
                    .find(filter.clone())
                    .skip(skip)
                    .limit(i64::from(self.results_per_page))
                    .await
                    .map_err(|error| {
                        EngineError::Message(format!("mongodb: query failed: {error}"))
                    })?;
                let mut results = Vec::new();
                while let Some(document) = cursor.try_next().await.map_err(|error| {
                    EngineError::Message(format!("mongodb: cursor failed: {error}"))
                })? {
                    let mut map = BTreeMap::new();
                    for (key, value) in document.iter() {
                        if key == "_id" {
                            continue;
                        }
                        map.insert(key.clone(), bson_value_text(value));
                    }
                    results.push(row_to_item(results.len(), map));
                }
                Ok::<Vec<EngineItem>, EngineError>(results)
            };
            tokio::time::timeout(timeout, future)
                .await
                .map_err(|_| EngineError::Message("mongodb: query timed out".into()))?
        })
    }
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!(".*{escaped}.*")
}

fn bson_value_text(value: &bson::Bson) -> String {
    match value {
        bson::Bson::String(text) => text.clone(),
        bson::Bson::Int32(number) => number.to_string(),
        bson::Bson::Int64(number) => number.to_string(),
        bson::Bson::Double(number) => number.to_string(),
        bson::Bson::Boolean(flag) => flag.to_string(),
        bson::Bson::ObjectId(id) => id.to_hex(),
        bson::Bson::DateTime(date) => date.to_string(),
        other => other.to_string(),
    }
}

fn row_to_item(index: usize, row: BTreeMap<String, String>) -> EngineItem {
    let title = ["title", "name", "label", "valkey_key"]
        .iter()
        .find_map(|key| row.get(*key).filter(|value| !value.is_empty()).cloned())
        .or_else(|| row.values().find(|value| !value.is_empty()).cloned())
        .unwrap_or_else(|| format!("row {}", index + 1));
    let mut builder =
        ResultBuilder::new(format!("keyvalue:{index}"), title).template(Template::KeyValue);
    for (key, value) in &row {
        builder = builder.metadata(key, value);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("mongodb", "mongodb", "", &["general"]);
        for (key, value) in [
            ("database", "searx"),
            ("collection", "pages"),
            ("key", "title"),
        ] {
            config
                .extra
                .insert(key.into(), toml::Value::String(value.into()));
        }
        config
    }

    #[test]
    fn requires_database_collection_and_key() {
        let bare = EngineConfig::new("mongodb", "mongodb", "", &["general"]);
        assert!(Mongodb::from_config(&bare).is_err());
    }

    #[test]
    fn builds_the_connection_uri() {
        let engine = Mongodb::from_config(&config()).expect("engine");
        assert_eq!(engine.connection_uri(), "mongodb://127.0.0.1:27017");
        assert_eq!(engine.results_per_page, 20);
        assert!(engine.is_offline());
    }

    #[test]
    fn escapes_regex_metacharacters() {
        assert_eq!(regex_escape("a.b"), ".*a\\.b.*");
        assert_eq!(regex_escape("x+y"), ".*x\\+y.*");
    }

    #[test]
    fn rows_become_keyvalue_results() {
        let mut row = BTreeMap::new();
        row.insert("title".to_string(), "Doc".to_string());
        match row_to_item(0, row) {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Doc");
                assert_eq!(result.template, Template::KeyValue);
            }
            _ => panic!("expected a result"),
        }
    }
}
