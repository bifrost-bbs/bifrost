use std::sync::{Arc, Mutex};
use std::path::Path;
use rusqlite::{Connection, Result as SqlResult, params, OptionalExtension};

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

fn default_db_path() -> String {
    "database.db".to_string()
}

pub fn default_database_config() -> DatabaseConfig {
    DatabaseConfig {
        path: default_db_path(),
    }
}

pub struct DatabaseStore {
    conn: Arc<Mutex<Connection>>,
}

impl Clone for DatabaseStore {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
        }
    }
}

impl DatabaseStore {
    pub fn new<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS store (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT,
                PRIMARY KEY (namespace, key)
            )",
            [],
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn new_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS store (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT,
                PRIMARY KEY (namespace, key)
            )",
            [],
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn get(&self, namespace: &str, key: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM store WHERE namespace = ?1 AND key = ?2")?;
        let res = stmt.query_row(params![namespace, key], |row| row.get(0)).optional()?;
        Ok(res)
    }

    pub fn set(&self, namespace: &str, key: &str, value: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO store (namespace, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(namespace, key) DO UPDATE SET value = ?3",
            params![namespace, key, value],
        )?;
        Ok(())
    }

    pub fn remove(&self, namespace: &str, key: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM store WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        Ok(())
    }

    pub fn keys(&self, namespace: &str) -> SqlResult<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key FROM store WHERE namespace = ?1")?;
        let rows = stmt.query_map(params![namespace], |row| row.get(0))?;
        let mut keys = Vec::new();
        for key in rows {
            keys.push(key?);
        }
        Ok(keys)
    }

    pub fn table_exists(&self, namespace: &str) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT 1 FROM store WHERE namespace = ?1 LIMIT 1")?;
        let exists = stmt.query_row(params![namespace], |_| Ok(true)).optional()?.unwrap_or(false);
        Ok(exists)
    }
}
