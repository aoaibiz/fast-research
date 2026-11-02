//! MySQL engine, ported from searx/engines/mysql_server.py.
//!
//! An offline engine: it runs the configured SELECT statement against a MySQL
//! server and returns the rows as key/value results.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::util::ResultBuilder;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: mysql_server

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

/// Connection settings shared by the MySQL and MariaDB engines.
pub(crate) struct SqlConnection {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) query_str: String,
    pub(crate) limit: i64,
}

impl SqlConnection {
    pub(crate) fn from_meta(meta: &EngineMeta, engine: &str) -> Result<Self, String> {
        let query_str = meta.opt_str("query_str").unwrap_or("").trim().to_string();
        if query_str.is_empty() {
            return Err(format!("{engine} requires the query_str option"));
        }
        if !query_str.to_lowercase().starts_with("select ") {
            return Err(format!("{engine} only supports SELECT statements"));
        }
        Ok(Self {
            host: meta.opt_str("host").unwrap_or("127.0.0.1").to_string(),
            port: meta
                .opt_i64("port")
                .and_then(|value| u16::try_from(value).ok())
                .unwrap_or(3306),
            database: meta.opt_str("database").unwrap_or("").to_string(),
            username: meta.opt_str("username").unwrap_or("").to_string(),
            password: meta.opt_str("password").unwrap_or("").to_string(),
            query_str,
            limit: meta.opt_i64("limit").unwrap_or(10).max(1),
        })
    }

    /// Run the configured query and return the rows.
    pub(crate) fn rows(
        &self,
        query: &str,
        pageno: u32,
    ) -> Result<Vec<BTreeMap<String, String>>, String> {
        use mysql::prelude::Queryable;
        let options = mysql::OptsBuilder::new()
            .ip_or_hostname(Some(self.host.clone()))
            .tcp_port(self.port)
            .db_name(Some(self.database.clone()))
            .user(Some(self.username.clone()))
            .pass(Some(self.password.clone()));
        let pool =
            mysql::Pool::new(options).map_err(|error| format!("mysql: cannot connect: {error}"))?;
        let mut connection = pool
            .get_conn()
            .map_err(|error| format!("mysql: cannot connect: {error}"))?;
        let offset = i64::from(pageno.saturating_sub(1)) * self.limit;
        let statement = format!("{} LIMIT {} OFFSET {}", self.query_str, self.limit, offset);
        let rows: Vec<mysql::Row> = connection
            .exec(statement.as_str(), (query,))
            .map_err(|error| format!("mysql: query failed: {error}"))?;
        let mut collected = Vec::new();
        for row in rows {
            let mut map = BTreeMap::new();
            for (index, column) in row.columns_ref().iter().enumerate() {
                let value = row
                    .as_ref(index)
                    .and_then(|value| mysql::from_value_opt::<String>(value.clone()).ok())
                    .unwrap_or_default();
                map.insert(column.name_str().to_string(), value);
            }
            collected.push(map);
        }
        Ok(collected)
    }
}

/// A MySQL backed engine.
pub struct MysqlServer {
    meta: EngineMeta,
    connection: SqlConnection,
}

impl MysqlServer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let connection = SqlConnection::from_meta(&meta, "mysql_server")?;
        Ok(Arc::new(Self { meta, connection }))
    }
}

impl Engine for MysqlServer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "mysql_server is an offline engine".into(),
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
        let rows = self
            .connection
            .rows(&query.query_text(), pageno)
            .map_err(EngineError::Message)?;
        Ok(rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| row_to_item(index, row))
            .collect())
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
        let mut config = EngineConfig::new("mysql", "mysql_server", "", &["general"]);
        config.extra.insert(
            "query_str".into(),
            toml::Value::String("SELECT * FROM docs WHERE body = ?".into()),
        );
        config
    }

    #[test]
    fn requires_a_select_statement() {
        let bare = EngineConfig::new("mysql", "mysql_server", "", &["general"]);
        assert!(MysqlServer::from_config(&bare).is_err());
    }

    #[test]
    fn reads_the_connection_settings() {
        let engine = MysqlServer::from_config(&config()).expect("engine");
        assert_eq!(engine.connection.host, "127.0.0.1");
        assert_eq!(engine.connection.port, 3306);
        assert!(engine.is_offline());
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
