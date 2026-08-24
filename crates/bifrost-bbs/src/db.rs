use std::sync::{Arc, Mutex};
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableStats {
    pub namespace: String,
    pub count: usize,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DbTelemetryStats {
    pub total_queries: u64,
    pub read_queries: u64,
    pub write_queries: u64,
    pub queries_per_hour: f64,
    pub avg_query_time_micros: f64,
    pub min_query_time_micros: u64,
    pub max_query_time_micros: u64,
    pub db_size_bytes: u64,
    pub total_records: usize,
    pub byte_growth_per_day: f64,
    pub record_growth_per_day: f64,
}

#[derive(Debug)]
pub struct DatabaseMetrics {
    query_count: std::sync::atomic::AtomicU64,
    read_queries: std::sync::atomic::AtomicU64,
    write_queries: std::sync::atomic::AtomicU64,
    total_query_nanos: std::sync::atomic::AtomicU64,
    min_query_nanos: std::sync::atomic::AtomicU64,
    max_query_nanos: std::sync::atomic::AtomicU64,
    started_at: std::time::Instant,
    history: Mutex<Vec<(std::time::Instant, usize, u64)>>,
}

impl Default for DatabaseMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseMetrics {
    pub fn new() -> Self {
        Self {
            query_count: std::sync::atomic::AtomicU64::new(0),
            read_queries: std::sync::atomic::AtomicU64::new(0),
            write_queries: std::sync::atomic::AtomicU64::new(0),
            total_query_nanos: std::sync::atomic::AtomicU64::new(0),
            min_query_nanos: std::sync::atomic::AtomicU64::new(u64::MAX),
            max_query_nanos: std::sync::atomic::AtomicU64::new(0),
            started_at: std::time::Instant::now(),
            history: Mutex::new(Vec::new()),
        }
    }

    pub fn record_query(&self, duration_nanos: u64, is_write: bool) {
        self.query_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if is_write {
            self.write_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.read_queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.total_query_nanos.fetch_add(duration_nanos, std::sync::atomic::Ordering::Relaxed);

        let mut cur_min = self.min_query_nanos.load(std::sync::atomic::Ordering::Relaxed);
        while duration_nanos < cur_min {
            match self.min_query_nanos.compare_exchange_weak(
                cur_min,
                duration_nanos,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur_min = actual,
            }
        }

        let mut cur_max = self.max_query_nanos.load(std::sync::atomic::Ordering::Relaxed);
        while duration_nanos > cur_max {
            match self.max_query_nanos.compare_exchange_weak(
                cur_max,
                duration_nanos,
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur_max = actual,
            }
        }
    }

    pub fn snapshot_growth(&self, records: usize, bytes: u64) {
        let mut hist = self.history.lock().unwrap();
        let now = std::time::Instant::now();
        hist.push((now, records, bytes));
        if hist.len() > 120 {
            hist.remove(0);
        }
    }

    pub fn compute_telemetry(&self, total_records: usize, db_size_bytes: u64) -> DbTelemetryStats {
        let count = self.query_count.load(std::sync::atomic::Ordering::Relaxed);
        let read = self.read_queries.load(std::sync::atomic::Ordering::Relaxed);
        let write = self.write_queries.load(std::sync::atomic::Ordering::Relaxed);
        let total_nanos = self.total_query_nanos.load(std::sync::atomic::Ordering::Relaxed);
        let min_nanos = self.min_query_nanos.load(std::sync::atomic::Ordering::Relaxed);
        let max_nanos = self.max_query_nanos.load(std::sync::atomic::Ordering::Relaxed);

        let elapsed_secs = self.started_at.elapsed().as_secs_f64().max(1.0);
        let queries_per_hour = (count as f64 / elapsed_secs) * 3600.0;

        let avg_query_time_micros = if count > 0 {
            (total_nanos as f64 / count as f64) / 1000.0
        } else {
            0.0
        };

        let min_query_time_micros = if min_nanos == u64::MAX { 0 } else { min_nanos / 1000 };
        let max_query_time_micros = max_nanos / 1000;

        let (byte_growth_per_day, record_growth_per_day) = {
            let hist = self.history.lock().unwrap();
            if hist.len() >= 2 {
                let first = &hist[0];
                let last = &hist[hist.len() - 1];
                let diff_time = last.0.duration_since(first.0).as_secs_f64().max(1.0);
                let diff_bytes = (last.2 as f64 - first.2 as f64).max(0.0);
                let diff_records = (last.1 as f64 - first.1 as f64).max(0.0);
                (
                    (diff_bytes / diff_time) * 86400.0,
                    (diff_records / diff_time) * 86400.0,
                )
            } else {
                (0.0, 0.0)
            }
        };

        DbTelemetryStats {
            total_queries: count,
            read_queries: read,
            write_queries: write,
            queries_per_hour,
            avg_query_time_micros,
            min_query_time_micros,
            max_query_time_micros,
            db_size_bytes,
            total_records,
            byte_growth_per_day,
            record_growth_per_day,
        }
    }
}

pub struct DatabaseStore {
    conn: Arc<Mutex<Connection>>,
    db_path: Option<PathBuf>,
    pub metrics: Arc<DatabaseMetrics>,
}

impl Clone for DatabaseStore {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            db_path: self.db_path.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

impl DatabaseStore {
    pub fn new<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let p_buf = path.as_ref().to_path_buf();
        let conn = Connection::open(&p_buf)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS store (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT,
                PRIMARY KEY (namespace, key)
            )",
            [],
        )?;
        let metrics = Arc::new(DatabaseMetrics::new());
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(p_buf),
            metrics,
        };
        let _ = store.auto_migrate_monolithic_rows();
        let records = store.total_records().unwrap_or(0);
        let size = store.db_size_bytes();
        store.metrics.snapshot_growth(records, size);
        Ok(store)
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
        let metrics = Arc::new(DatabaseMetrics::new());
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: None,
            metrics,
        };
        let _ = store.auto_migrate_monolithic_rows();
        let records = store.total_records().unwrap_or(0);
        let size = store.db_size_bytes();
        store.metrics.snapshot_growth(records, size);
        Ok(store)
    }

    pub fn get(&self, namespace: &str, key: &str) -> SqlResult<Option<String>> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM store WHERE namespace = ?1 AND key = ?2")?;
        let res = stmt.query_row(params![namespace, key], |row| row.get(0)).optional()?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(res)
    }

    pub fn set(&self, namespace: &str, key: &str, value: &str) -> SqlResult<()> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO store (namespace, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(namespace, key) DO UPDATE SET value = ?3",
            params![namespace, key, value],
        )?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, true);
        Ok(())
    }

    pub fn set_batch(&self, namespace: &str, entries: &[(String, String)]) -> SqlResult<()> {
        let start = std::time::Instant::now();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO store (namespace, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(namespace, key) DO UPDATE SET value = ?3",
            )?;
            for (k, v) in entries {
                stmt.execute(params![namespace, k, v])?;
            }
        }
        tx.commit()?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, true);
        Ok(())
    }

    pub fn remove(&self, namespace: &str, key: &str) -> SqlResult<()> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM store WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, true);
        Ok(())
    }

    pub fn keys(&self, namespace: &str) -> SqlResult<Vec<String>> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key FROM store WHERE namespace = ?1 ORDER BY CASE WHEN key GLOB '[0-9]*' THEN CAST(key AS INTEGER) ELSE NULL END ASC, key ASC"
        )?;
        let rows = stmt.query_map(params![namespace], |row| row.get(0))?;
        let mut keys = Vec::new();
        for key in rows {
            keys.push(key?);
        }
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(keys)
    }

    pub fn table_exists(&self, namespace: &str) -> SqlResult<bool> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT 1 FROM store WHERE namespace = ?1 LIMIT 1")?;
        let exists = stmt.query_row(params![namespace], |_| Ok(true)).optional()?.unwrap_or(false);
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(exists)
    }

    pub fn get_all(&self, namespace: &str) -> SqlResult<Vec<(String, String)>> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT key, value FROM store WHERE namespace = ?1 ORDER BY CASE WHEN key GLOB '[0-9]*' THEN CAST(key AS INTEGER) ELSE NULL END ASC, key ASC"
        )?;
        let rows = stmt.query_map(params![namespace], |row| {
            let k: String = row.get(0)?;
            let v: Option<String> = row.get(1)?;
            Ok((k, v.unwrap_or_default()))
        })?;
        let mut entries = Vec::new();
        for entry in rows {
            entries.push(entry?);
        }
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(entries)
    }

    pub fn namespaces(&self) -> SqlResult<Vec<String>> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT DISTINCT namespace FROM store ORDER BY namespace ASC")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut ns = Vec::new();
        for n in rows {
            ns.push(n?);
        }
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(ns)
    }

    pub fn clear_namespace(&self, namespace: &str) -> SqlResult<usize> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let count = conn.execute("DELETE FROM store WHERE namespace = ?1", params![namespace])?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, true);
        Ok(count)
    }

    pub fn count(&self, namespace: &str) -> SqlResult<usize> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM store WHERE namespace = ?1")?;
        let count: i64 = stmt.query_row(params![namespace], |row| row.get(0))?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(count as usize)
    }

    pub fn total_records(&self) -> SqlResult<usize> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT COUNT(*) FROM store")?;
        let count: i64 = stmt.query_row([], |row| row.get(0))?;
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(count as usize)
    }

    pub fn table_stats(&self) -> SqlResult<Vec<TableStats>> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT namespace, COUNT(*), SUM(LENGTH(key) + COALESCE(LENGTH(value), 0))
             FROM store
             GROUP BY namespace
             ORDER BY namespace ASC"
        )?;
        let rows = stmt.query_map([], |row| {
            let ns: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let size: Option<i64> = row.get(2)?;
            Ok(TableStats {
                namespace: ns,
                count: count as usize,
                size_bytes: size.unwrap_or(0).max(0) as u64,
            })
        })?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        self.metrics.record_query(start.elapsed().as_nanos() as u64, false);
        Ok(list)
    }

    pub fn db_size_bytes(&self) -> u64 {
        if let Some(ref path) = self.db_path {
            std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
        } else {
            let conn = self.conn.lock().unwrap();
            let sum: Option<i64> = conn
                .query_row(
                    "SELECT SUM(LENGTH(namespace) + LENGTH(key) + COALESCE(LENGTH(value), 0)) FROM store",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(None);
            sum.unwrap_or(0).max(0) as u64
        }
    }

    pub fn telemetry_stats(&self) -> DbTelemetryStats {
        let records = self.total_records().unwrap_or(0);
        let size = self.db_size_bytes();
        self.metrics.snapshot_growth(records, size);
        self.metrics.compute_telemetry(records, size)
    }

    pub fn reset_database(&self) -> SqlResult<()> {
        let start = std::time::Instant::now();
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM store", [])?;
        conn.execute("VACUUM", []).ok();
        self.metrics.record_query(start.elapsed().as_nanos() as u64, true);
        Ok(())
    }

    pub fn backup_bytes(&self) -> anyhow::Result<Vec<u8>> {
        if let Some(ref path) = self.db_path {
            if path.exists() {
                return Ok(std::fs::read(path)?);
            }
        }
        let conn = self.conn.lock().unwrap();
        let temp_path = std::env::temp_dir().join(format!(
            "backup_temp_{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        conn.execute("VACUUM INTO ?1", params![temp_path.to_str().unwrap()])?;
        let bytes = std::fs::read(&temp_path)?;
        let _ = std::fs::remove_file(temp_path);
        Ok(bytes)
    }

    pub fn restore_from_bytes(&self, data: &[u8]) -> anyhow::Result<()> {
        if let Some(ref path) = self.db_path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, data)?;
            let new_conn = Connection::open(path)?;
            let mut conn = self.conn.lock().unwrap();
            *conn = new_conn;
        } else {
            let temp_path = std::env::temp_dir().join(format!(
                "restore_temp_{}.db",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::write(&temp_path, data)?;
            let src = Connection::open(&temp_path)?;
            let mut conn = self.conn.lock().unwrap();
            *conn = Connection::open_in_memory()?;
            conn.execute(
                "CREATE TABLE IF NOT EXISTS store (namespace TEXT NOT NULL, key TEXT NOT NULL, value TEXT, PRIMARY KEY (namespace, key))",
                [],
            )?;
            let mut stmt = src.prepare("SELECT namespace, key, value FROM store")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            for r in rows {
                let (ns, k, v) = r?;
                conn.execute(
                    "INSERT OR REPLACE INTO store (namespace, key, value) VALUES (?1, ?2, ?3)",
                    params![ns, k, v],
                )?;
            }
            let _ = std::fs::remove_file(temp_path);
        }
        let _ = self.auto_migrate_monolithic_rows();
        Ok(())
    }

    pub fn auto_migrate_monolithic_rows(&self) -> SqlResult<()> {
        let namespaces = self.namespaces()?;
        for ns in namespaces {
            if let Some(val) = self.get(&ns, "all")? {
                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&val) {
                    let mut batch = Vec::new();
                    if let Some(arr) = json_val.as_array() {
                        if arr.len() > 1 {
                            for (idx, item) in arr.iter().enumerate() {
                                if let Ok(s) = serde_json::to_string(item) {
                                    batch.push(((idx + 1).to_string(), s));
                                }
                            }
                        }
                    } else if let Some(map) = json_val.as_object() {
                        if map.len() > 1 {
                            for (k, item) in map {
                                if let Ok(s) = serde_json::to_string(item) {
                                    batch.push((k.clone(), s));
                                }
                            }
                        }
                    }

                    if !batch.is_empty() {
                        self.remove(&ns, "all")?;
                        self.set_batch(&ns, &batch)?;
                    }
                }
            }
        }
        Ok(())
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

    #[test]
    fn test_database_store_set_batch() {
        let db = DatabaseStore::new_in_memory().unwrap();
        let batch = vec![
            ("1".to_string(), r#"{"name":"Sol"}"#.to_string()),
            ("2".to_string(), r#"{"name":"Alpha Centauri"}"#.to_string()),
            ("10".to_string(), r#"{"name":"Sirius"}"#.to_string()),
        ];
        db.set_batch("sectors", &batch).unwrap();

        assert_eq!(db.count("sectors").unwrap(), 3);
        let keys = db.keys("sectors").unwrap();
        assert_eq!(keys, vec!["1", "2", "10"]);

        let all = db.get_all("sectors").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].0, "1");
        assert_eq!(all[1].0, "2");
        assert_eq!(all[2].0, "10");
    }

    #[test]
    fn test_database_store_auto_migrate_monolithic_rows() {
        let db = DatabaseStore::new_in_memory().unwrap();
        // Insert a legacy monolithic row with key "all"
        let legacy_json = r#"[{"name":"Sol"},{"name":"Alpha"},{"name":"Vega"}]"#;
        db.set("vt_sectors", "all", legacy_json).unwrap();
        assert_eq!(db.count("vt_sectors").unwrap(), 1);

        // Run auto migration
        db.auto_migrate_monolithic_rows().unwrap();

        // Must now have 3 individual rows and key "all" removed
        assert_eq!(db.count("vt_sectors").unwrap(), 3);
        assert_eq!(db.get("vt_sectors", "all").unwrap(), None);
        let keys = db.keys("vt_sectors").unwrap();
        assert_eq!(keys, vec!["1", "2", "3"]);
        let s2 = db.get("vt_sectors", "2").unwrap();
        assert_eq!(s2, Some(r#"{"name":"Alpha"}"#.to_string()));
    }

    #[test]
    fn test_database_store_table_stats_and_telemetry() {
        let db = DatabaseStore::new_in_memory().unwrap();
        db.set("users", "node1", r#"{"nickname":"Alice"}"#).unwrap();
        db.set("users", "node2", r#"{"nickname":"Bob"}"#).unwrap();
        db.set("sectors", "1", r#"{"name":"Sol"}"#).unwrap();

        let stats = db.table_stats().unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].namespace, "sectors");
        assert_eq!(stats[0].count, 1);
        assert!(stats[0].size_bytes > 0);
        assert_eq!(stats[1].namespace, "users");
        assert_eq!(stats[1].count, 2);

        let tel = db.telemetry_stats();
        assert_eq!(tel.total_records, 3);
        assert!(tel.total_queries >= 3);
        assert_eq!(tel.write_queries, 3);
    }

    #[test]
    fn test_database_store_reset_and_backup_restore() {
        let temp_dir = std::env::temp_dir().join(format!(
            "bifrost_db_backup_test_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let db_path = temp_dir.join("test.db");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let db = DatabaseStore::new(&db_path).unwrap();
        db.set("users", "admin", r#"{"nickname":"Root"}"#).unwrap();
        db.set("scores", "p1", "1000").unwrap();
        assert_eq!(db.total_records().unwrap(), 2);

        // Backup
        let backup = db.backup_bytes().unwrap();
        assert!(!backup.is_empty());

        // Reset
        db.reset_database().unwrap();
        assert_eq!(db.total_records().unwrap(), 0);

        // Restore
        db.restore_from_bytes(&backup).unwrap();
        assert_eq!(db.total_records().unwrap(), 2);
        assert_eq!(db.get("users", "admin").unwrap(), Some(r#"{"nickname":"Root"}"#.to_string()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
