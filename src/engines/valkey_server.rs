//! Valkey / Redis engine, ported from searx/engines/valkey_server.py.
//!
//! Talks the RESP protocol directly, so no client dependency is needed.
//! Upstream is an offline engine: it looks keys up in a Valkey/Redis instance.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

// engine-id: valkey_server

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["general"],
    shortcut: "",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

/// A Valkey / Redis key value store.
pub struct ValkeyServer {
    meta: EngineMeta,
    host: String,
    port: u16,
    password: String,
    db: i64,
    exact_match_only: bool,
}

impl ValkeyServer {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        let meta = EngineMeta::from_config(config, DEFAULTS);
        let host = meta.opt_str("host").unwrap_or("127.0.0.1").to_string();
        let port = meta
            .opt_i64("port")
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(6379);
        let password = meta.opt_str("password").unwrap_or("").to_string();
        let db = meta.opt_i64("db").unwrap_or(0);
        let exact_match_only = meta.opt_bool("exact_match_only").unwrap_or(true);
        Ok(Arc::new(Self {
            meta,
            host,
            port,
            password,
            db,
            exact_match_only,
        }))
    }

    fn connect(&self) -> Result<Connection, EngineError> {
        let timeout = self.meta.timeout.max(Duration::from_millis(100));
        let stream = TcpStream::connect((self.host.as_str(), self.port)).map_err(|error| {
            EngineError::Message(format!(
                "valkey: cannot connect to {}:{}: {error}",
                self.host, self.port
            ))
        })?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(timeout)).ok();
        let mut connection = Connection {
            reader: BufReader::new(
                stream
                    .try_clone()
                    .map_err(|error| EngineError::Message(format!("valkey: {error}")))?,
            ),
            writer: stream,
        };
        if !self.password.is_empty() {
            connection.command(&["AUTH", &self.password])?;
        }
        if self.db != 0 {
            let db = self.db.to_string();
            connection.command(&["SELECT", &db])?;
        }
        Ok(connection)
    }

    /// Every key/value row, like upstream search_keys().
    fn search_keys(&self, query: &str) -> Result<Vec<BTreeMap<String, String>>, EngineError> {
        let mut connection = self.connect()?;
        let pattern = format!("*{query}*");
        let reply = connection.command(&["SCAN", "0", "MATCH", &pattern, "COUNT", "100"])?;
        let Reply::Array(items) = reply else {
            return Ok(Vec::new());
        };
        let Some(Reply::Array(key_list)) = items.get(1) else {
            return Ok(Vec::new());
        };
        let mut rows = Vec::new();
        for key in key_list {
            let Reply::Bulk(Some(key)) = key else {
                continue;
            };
            let kind = match connection.command(&["TYPE", key])? {
                Reply::Simple(value) | Reply::Bulk(Some(value)) => value,
                _ => continue,
            };
            let mut map = BTreeMap::new();
            match kind.as_str() {
                "hash" => {
                    if let Reply::Array(pairs) = connection.command(&["HGETALL", key])? {
                        for chunk in pairs.chunks(2) {
                            if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                                map.insert(name.clone(), value.clone());
                            }
                        }
                    }
                }
                "list" => {
                    if let Reply::Array(items) = connection.command(&["LRANGE", key, "0", "-1"])? {
                        for (index, item) in items.iter().enumerate() {
                            if let Reply::Bulk(Some(value)) = item {
                                map.insert(index.to_string(), value.clone());
                            }
                        }
                    }
                }
                _ => continue,
            }
            if !map.is_empty() {
                map.insert("valkey_key".to_string(), key.clone());
                rows.push(map);
            }
        }
        Ok(rows)
    }
}

impl Engine for ValkeyServer {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        Err(EngineError::Message(
            "valkey_server is an offline engine".into(),
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

    fn search_offline(&self, query: &Query, _pageno: u32) -> Result<Vec<EngineItem>, EngineError> {
        let text = query.query_text();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let mut connection = self.connect()?;
        if !self.exact_match_only {
            return Ok(rows_to_items(self.search_keys(&text)?));
        }
        let mut rows = Vec::new();
        if let Reply::Array(pairs) = connection.command(&["HGETALL", &text])? {
            let mut map = BTreeMap::new();
            for chunk in pairs.chunks(2) {
                if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                    map.insert(name.clone(), value.clone());
                }
            }
            if !map.is_empty() {
                rows.push(map);
            }
        }
        if rows.is_empty() {
            if let Some((set, rest)) = text.split_once(' ') {
                let pattern = format!("*{rest}*");
                if let Reply::Array(entries) =
                    connection.command(&["HSCAN", set, "0", "MATCH", &pattern])?
                {
                    if let Some(Reply::Array(pairs)) = entries.get(1) {
                        for chunk in pairs.chunks(2) {
                            if let [Reply::Bulk(Some(name)), Reply::Bulk(Some(value))] = chunk {
                                let mut map = BTreeMap::new();
                                map.insert(name.clone(), value.clone());
                                rows.push(map);
                            }
                        }
                    }
                }
            }
        }
        Ok(rows_to_items(rows))
    }
}

fn rows_to_items(rows: Vec<BTreeMap<String, String>>) -> Vec<EngineItem> {
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let title = row
                .get("valkey_key")
                .cloned()
                .or_else(|| row.keys().next().cloned())
                .unwrap_or_else(|| format!("row {}", index + 1));
            let mut builder =
                crate::engine::util::ResultBuilder::new(format!("keyvalue:{index}"), title)
                    .template(Template::KeyValue);
            for (key, value) in &row {
                builder = builder.metadata(key, value);
            }
            builder.build()
        })
        .collect()
}

/// One RESP reply.
#[derive(Debug, Clone)]
enum Reply {
    Simple(String),
    Bulk(Option<String>),
    #[allow(dead_code)]
    Integer(i64),
    Array(Vec<Reply>),
    Nil,
}

struct Connection {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Connection {
    fn command(&mut self, args: &[&str]) -> Result<Reply, EngineError> {
        let mut request = format!("*{}\r\n", args.len());
        for argument in args {
            request.push_str(&format!("${}$\r\n{}\r\n", argument.len(), argument));
        }
        self.writer
            .write_all(request.as_bytes())
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        self.writer
            .flush()
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        self.read_reply()
    }

    fn read_reply(&mut self) -> Result<Reply, EngineError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
        if line.is_empty() {
            return Err(EngineError::Message("valkey: connection closed".into()));
        }
        let marker = line.chars().next().unwrap_or(' ');
        let rest = line[1..].trim_end().to_string();
        Ok(match marker {
            '+' => Reply::Simple(rest),
            '-' => return Err(EngineError::Message(format!("valkey: {rest}"))),
            ':' => Reply::Integer(rest.parse().unwrap_or(0)),
            '$' => {
                let length: i64 = rest.parse().unwrap_or(-1);
                if length < 0 {
                    Reply::Nil
                } else {
                    let mut buffer = vec![0u8; length as usize + 2];
                    self.reader
                        .read_exact(&mut buffer)
                        .map_err(|error| EngineError::Message(format!("valkey: {error}")))?;
                    buffer.truncate(length as usize);
                    Reply::Bulk(Some(String::from_utf8_lossy(&buffer).to_string()))
                }
            }
            '*' => {
                let count: i64 = rest.parse().unwrap_or(0);
                let mut items = Vec::new();
                for _ in 0..count.max(0) {
                    items.push(self.read_reply()?);
                }
                Reply::Array(items)
            }
            _ => Reply::Nil,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<ValkeyServer> {
        let mut config = EngineConfig::new("valkey", "valkey_server", "", &["general"]);
        config
            .extra
            .insert("host".into(), toml::Value::String("127.0.0.1".into()));
        ValkeyServer::from_config(&config).expect("engine")
    }

    #[test]
    fn reads_the_connection_options() {
        let engine = engine();
        assert_eq!(engine.host, "127.0.0.1");
        assert_eq!(engine.port, 6379);
        assert!(engine.exact_match_only);
        assert!(engine.is_offline());
    }

    #[test]
    fn rows_become_keyvalue_results() {
        let mut row = BTreeMap::new();
        row.insert("valkey_key".to_string(), "user:1".to_string());
        row.insert("name".to_string(), "Ada".to_string());
        let items = rows_to_items(vec![row]);
        assert_eq!(items.len(), 1);
        match &items[0] {
            EngineItem::Result(result) => {
                assert_eq!(result.title, "user:1");
                assert_eq!(result.template, Template::KeyValue);
                assert_eq!(result.metadata.get("name").map(String::as_str), Some("Ada"));
            }
            _ => panic!("expected a result"),
        }
    }
}
