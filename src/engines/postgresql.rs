//! PostgreSQL engine, ported from searx/engines/postgresql.py.
//!
//! An offline engine: it runs the configured SELECT statement against a
//! PostgreSQL database and returns the rows as key/value results.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::util::ResultBuilder;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: postgresql

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

/// A PostgreSQL connection description.
pub struct Postgresql {
    meta: EngineMeta,
    host: String,
    port: u16,
    database: String,
    username: String,
    password: String,
    query_str: String,
    limit: i64,
}

impl Postgresql {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let query_str = meta.opt_str("query_str").unwrap_or("").trim().to_string();
        if query_str.is_empty() {
            return Err("postgresql requires the query_str option".into());
        }
        if !query_str.to_lowercase().starts_with("select ") {
            return Err("postgresql only supports SELECT statements".into());
        }
        let host = meta.opt_str("host").unwrap_or("127.0.0.1").to_string();
        let port = meta
            .opt_i64("port")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(5432);
        let database = meta.opt_str("database").unwrap_or("").to_string();
        let username = meta.opt_str("username").unwrap_or("").to_string();
        let password = meta.opt_str("password").unwrap_or("").to_string();
        let limit = meta.opt_i64("limit").unwrap_or(10).max(1);
        Ok(Arc::new(Self {
            meta,
            host,
            port,
            database,
            username,
            password,
            query_str,
            limit,
        }))
    }

    fn connection_string(&self) -> String {
        format!(
            "host={} port={} dbname={} user={} password={} connect_timeout=5",
            self.host, self.port, self.database, self.username, self.password
        )
    }
}

impl Engine for Postgresql {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "postgresql is an offline engine".into(),
        ))
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
        let offset = i64::from(pageno.saturating_sub(1)) * self.limit;
        let statement = format!("{} LIMIT {} OFFSET {}", self.query_str, self.limit, offset);
        let mut client = postgres::Client::connect(&self.connection_string(), postgres::NoTls)
            .map_err(|error| {
                EngineError::Message(format!("postgresql: cannot connect: {error}"))
            })?;
        let rows = client
            .query(&statement, &[&query.query_text()])
            .map_err(|error| EngineError::Message(format!("postgresql: query failed: {error}")))?;
        let mut results = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let mut map = BTreeMap::new();
            for (column_index, column) in row.columns().iter().enumerate() {
                let value = row
                    .try_get::<usize, Option<String>>(column_index)
                    .unwrap_or(None)
                    .unwrap_or_default();
                map.insert(column.name().to_string(), value);
            }
            results.push(row_to_item(index, map));
        }
        Ok(results)
    }
}

/// One row as a key/value result (upstream KeyValue result type).
pub(crate) fn row_to_item(index: usize, row: BTreeMap<String, String>) -> EngineItem {
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
        let mut config = EngineConfig::new("postgresql", "postgresql", "", &["general"]);
        config.extra.insert(
            "query_str".into(),
            toml::Value::String("SELECT * FROM docs WHERE body = $1".to_string()),
        );
        config
            .extra
            .insert("database".into(), toml::Value::String("searx".into()));
        config
    }

    #[test]
    fn requires_a_select_statement() {
        let bare = EngineConfig::new("postgresql", "postgresql", "", &["general"]);
        assert!(Postgresql::from_config(&bare).is_err());
        let mut update = bare;
        update.extra.insert(
            "query_str".into(),
            toml::Value::String("UPDATE docs SET body = 1".into()),
        );
        assert!(Postgresql::from_config(&update).is_err());
    }

    #[test]
    fn reads_the_connection_settings() {
        let engine = Postgresql::from_config(&config()).expect("engine");
        assert_eq!(engine.host, "127.0.0.1");
        assert_eq!(engine.port, 5432);
        assert_eq!(engine.limit, 10);
        assert!(engine.connection_string().contains("dbname=searx"));
        assert!(engine.is_offline());
    }

    #[test]
    fn rows_become_keyvalue_results() {
        let mut row = BTreeMap::new();
        row.insert("title".to_string(), "Doc".to_string());
        row.insert("url".to_string(), "https://example.test".to_string());
        let item = row_to_item(0, row);
        match item {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "Doc");
                assert_eq!(result.template, Template::KeyValue);
                assert_eq!(
                    result.metadata.get("url").map(String::as_str),
                    Some("https://example.test")
                );
            }
            _ => panic!("expected a result"),
        }
    }
}
