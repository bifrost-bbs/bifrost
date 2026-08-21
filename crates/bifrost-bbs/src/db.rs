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

    pub fn get_all(&self, namespace: &str) -> SqlResult<Vec<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM store WHERE namespace = ?1 ORDER BY key ASC")?;
        let rows = stmt.query_map(params![namespace], |row| {
            let k: String = row.get(0)?;
            let v: Option<String> = row.get(1)?;
            Ok((k, v.unwrap_or_default()))
        })?;
        let mut entries = Vec::new();
        for entry in rows {
            entries.push(entry?);
        }
        Ok(entries)
    }

    pub fn namespaces(&self) -> SqlResult<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT namespace FROM store ORDER BY namespace ASC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut ns = Vec::new();
        for n in rows {
            ns.push(n?);
        }
        Ok(ns)
    }

    pub fn clear_namespace(&self, namespace: &str) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute("DELETE FROM store WHERE namespace = ?1", params![namespace])?;
        Ok(count)
    }

    pub fn count(&self, namespace: &str) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM store WHERE namespace = ?1")?;
        let count: i64 = stmt.query_row(params![namespace], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn total_records(&self) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM store")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_store_get_set_remove() {
        let db = DatabaseStore::new_in_memory().unwrap();
        assert_eq!(db.get("users", "alice").unwrap(), None);

        db.set("users", "alice", "{\"score\": 100}").unwrap();
        assert_eq!(db.get("users", "alice").unwrap(), Some("{\"score\": 100}".to_string()));

        db.set("users", "alice", "{\"score\": 200}").unwrap();
        assert_eq!(db.get("users", "alice").unwrap(), Some("{\"score\": 200}".to_string()));

        db.remove("users", "alice").unwrap();
        assert_eq!(db.get("users", "alice").unwrap(), None);
    }

    #[test]
    fn test_database_store_keys_and_namespaces() {
        let db = DatabaseStore::new_in_memory().unwrap();
        db.set("players", "p1", "val1").unwrap();
        db.set("players", "p2", "val2").unwrap();
        db.set("market", "item1", "sword").unwrap();

        let mut keys = db.keys("players").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["p1", "p2"]);

        let ns = db.namespaces().unwrap();
        assert_eq!(ns, vec!["market", "players"]);

        assert!(db.table_exists("players").unwrap());
        assert!(!db.table_exists("nonexistent").unwrap());

        assert_eq!(db.count("players").unwrap(), 2);
        assert_eq!(db.count("market").unwrap(), 1);
        assert_eq!(db.total_records().unwrap(), 3);

        let all = db.get_all("players").unwrap();
        assert_eq!(all, vec![("p1".to_string(), "val1".to_string()), ("p2".to_string(), "val2".to_string())]);

        db.clear_namespace("players").unwrap();
        assert_eq!(db.count("players").unwrap(), 0);
        assert_eq!(db.total_records().unwrap(), 1);
    }

    #[test]
    fn test_database_store_persistence_on_disk() {
        let temp_dir = std::env::temp_dir().join(format!("bifrost_db_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let db_path = temp_dir.join("test.db");
        std::fs::create_dir_all(&temp_dir).unwrap();

        {
            let db = DatabaseStore::new(&db_path).unwrap();
            db.set("vault", "gold", "9999").unwrap();
        }

        // Reopen database
        {
            let db = DatabaseStore::new(&db_path).unwrap();
            assert_eq!(db.get("vault", "gold").unwrap(), Some("9999".to_string()));
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
