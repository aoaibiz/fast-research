//! MariaDB engine, ported from searx/engines/mariadb_server.py.
//!
//! MariaDB speaks the MySQL protocol, so the engine reuses the MySQL
//! connection code and only differs in its defaults and engine id.

use std::sync::Arc;

use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::settings::EngineConfig;

use super::mysql_server::{SqlConnection, row_to_item};

// engine-id: mariadb_server

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

/// A MariaDB backed engine.
pub struct MariadbServer {
    meta: EngineMeta,
    connection: SqlConnection,
}

impl MariadbServer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let connection = SqlConnection::from_meta(&meta, "mariadb_server")?;
        Ok(Arc::new(Self { meta, connection }))
    }
}

impl Engine for MariadbServer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "mariadb_server is an offline engine".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> EngineConfig {
        let mut config = EngineConfig::new("mariadb", "mariadb_server", "", &["general"]);
        config.extra.insert(
            "query_str".into(),
            toml::Value::String("SELECT * FROM docs WHERE body = ?".into()),
        );
        config
    }

    #[test]
    fn requires_a_select_statement() {
        let bare = EngineConfig::new("mariadb", "mariadb_server", "", &["general"]);
        assert!(MariadbServer::from_config(&bare).is_err());
    }

    #[test]
    fn shares_the_mysql_connection_code() {
        let engine = MariadbServer::from_config(&config()).expect("engine");
        assert_eq!(engine.connection.port, 3306);
        assert!(engine.is_offline());
    }
}
