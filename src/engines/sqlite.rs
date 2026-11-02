//! SQLite engine, ported from searx/engines/sqlite.py.
//!
//! An offline engine: it runs the configured SELECT statement against a local
//! SQLite database file and returns the rows as key/value results.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::util::ResultBuilder;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: sqlite

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

/// A SQLite database file.
pub struct Sqlite {
    meta: EngineMeta,
    database: String,
    query_str: String,
    limit: i64,
}

impl Sqlite {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let database = meta.opt_str("database").unwrap_or("").trim().to_string();
        if database.is_empty() {
            return Err("sqlite requires the database option".into());
        }
        let query_str = meta.opt_str("query_str").unwrap_or("").trim().to_string();
        if query_str.is_empty() {
            return Err("sqlite requires the query_str option".into());
        }
        if !query_str.to_lowercase().starts_with("select ") {
            return Err("sqlite only supports SELECT statements".into());
        }
        let limit = meta.opt_i64("limit").unwrap_or(10).max(1);
        Ok(Arc::new(Self {
            meta,
            database,
            query_str,
            limit,
        }))
    }
}

impl Engine for Sqlite {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message("sqlite is an offline engine".into()))
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
        let connection = rusqlite::Connection::open(&self.database).map_err(|error| {
            EngineError::Message(format!("sqlite: cannot open {}: {error}", self.database))
        })?;
        let offset = i64::from(pageno.saturating_sub(1)) * self.limit;
        let statement = format!(
            "{} LIMIT :limit OFFSET :offset",
            self.query_str.trim_end_matches(';')
        );
        let mut prepared = connection.prepare(&statement).map_err(|error| {
            EngineError::Message(format!("sqlite: cannot prepare the query: {error}"))
        })?;
        let columns: Vec<String> = prepared
            .column_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut rows = prepared
            .query(rusqlite::named_params! {
                ":query": query.query_text(),
                ":limit": self.limit,
                ":offset": offset,
            })
            .map_err(|error| EngineError::Message(format!("sqlite: query failed: {error}")))?;
        let mut results = Vec::new();
        let mut index = 0;
        while let Some(row) = rows
            .next()
            .map_err(|error| EngineError::Message(format!("sqlite: row error: {error}")))?
        {
            let mut map = BTreeMap::new();
            for (column_index, name) in columns.iter().enumerate() {
                let value = match row.get::<usize, rusqlite::types::Value>(column_index) {
                    Ok(rusqlite::types::Value::Null) | Err(_) => String::new(),
                    Ok(rusqlite::types::Value::Integer(number)) => number.to_string(),
                    Ok(rusqlite::types::Value::Real(number)) => number.to_string(),
                    Ok(rusqlite::types::Value::Text(text)) => text,
                    Ok(rusqlite::types::Value::Blob(blob)) => {
                        format!("<{} bytes>", blob.len())
                    }
                };
                if !value.is_empty() {
                    map.insert(name.clone(), value);
                }
            }
            results.push(row_to_item(index, map));
            index += 1;
        }
        Ok(results)
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

    fn engine(database: &str) -> Arc<Sqlite> {
        let mut config = EngineConfig::new("sqlite", "sqlite", "", &["general"]);
        config
            .extra
            .insert("database".into(), toml::Value::String(database.into()));
        config.extra.insert(
            "query_str".into(),
            toml::Value::String(
                "SELECT title, body FROM docs WHERE body LIKE '%' || :query || '%'".into(),
            ),
        );
        Sqlite::from_config(&config).expect("engine")
    }

    #[test]
    fn requires_a_database_and_a_select_statement() {
        let bare = EngineConfig::new("sqlite", "sqlite", "", &["general"]);
        assert!(Sqlite::from_config(&bare).is_err());
    }

    #[test]
    fn rows_become_keyvalue_results() {
        let directory = std::env::temp_dir().join(format!("rustxng-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("dir");
        let path = directory.join("test.db");
        let path_text = path.to_string_lossy().to_string();
        {
            let connection = rusqlite::Connection::open(&path).expect("open");
            connection
                .execute_batch(
                    "CREATE TABLE docs (title TEXT, body TEXT);
                     INSERT INTO docs VALUES ('Doc one', 'hello rust');
                     INSERT INTO docs VALUES ('Doc two', 'other');",
                )
                .expect("schema");
        }
        let engine = engine(&path_text);
        let query = Query::parse("rust", &crate::engine::EngineRegistry::default(), &[]);
        let items = engine.search_offline(&query, 1).expect("search");
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Doc one");
                assert_eq!(result.template, Template::KeyValue);
                assert!(result.metadata.contains_key("body"));
            }
            _ => panic!("expected a result"),
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}
