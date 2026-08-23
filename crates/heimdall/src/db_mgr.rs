//! SQLite Database manager and inspector for Heimdall.

use anyhow::{Context, Result};
use bifrost_bbs::{DatabaseStore, DbTelemetryStats};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSummary {
    pub namespace: String,
    pub count: usize,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValueEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSummary {
    pub path: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub total_tables: usize,
    pub total_records: usize,
    pub tables: Vec<TableSummary>,
    pub telemetry: DbTelemetryStats,
}

#[derive(Debug)]
pub struct DatabaseManager {
    db_path: PathBuf,
}

impl DatabaseManager {
    pub fn new(db_path: impl AsRef<Path>) -> Self {
        Self {
            db_path: db_path.as_ref().to_path_buf(),
        }
    }

    pub fn get_db_path(&self) -> &Path {
        &self.db_path
    }

    fn get_store(&self) -> Result<DatabaseStore> {
        if let Some(parent) = self.db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let store = DatabaseStore::new(&self.db_path)
            .with_context(|| format!("Failed to open SQLite database at {:?}", self.db_path))?;
        Ok(store)
    }

    pub fn summary(&self) -> Result<DatabaseSummary> {
        let exists = self.db_path.exists();
        if !exists {
            let in_mem = DatabaseStore::new_in_memory()?;
            return Ok(DatabaseSummary {
                path: self.db_path.to_string_lossy().to_string(),
                exists: false,
                size_bytes: 0,
                total_tables: 0,
                total_records: 0,
                tables: Vec::new(),
                telemetry: in_mem.telemetry_stats(),
            });
        }

        let store = self.get_store()?;
        let total_records = store.total_records().unwrap_or(0);
        let size_bytes = store.db_size_bytes();
        let telemetry = store.telemetry_stats();

        let table_stats = store.table_stats().unwrap_or_default();
        let tables: Vec<TableSummary> = table_stats
            .into_iter()
            .map(|t| TableSummary {
                namespace: t.namespace,
                count: t.count,
                size_bytes: t.size_bytes,
            })
            .collect();

        Ok(DatabaseSummary {
            path: self.db_path.to_string_lossy().to_string(),
            exists: true,
            size_bytes,
            total_tables: tables.len(),
            total_records,
            tables,
            telemetry,
        })
    }

    pub fn list_tables(&self) -> Result<Vec<TableSummary>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let store = self.get_store()?;
        let table_stats = store.table_stats().unwrap_or_default();
        Ok(table_stats
            .into_iter()
            .map(|t| TableSummary {
                namespace: t.namespace,
                count: t.count,
                size_bytes: t.size_bytes,
            })
            .collect())
    }

    pub fn get_table_entries(&self, namespace: &str) -> Result<Vec<KeyValueEntry>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let store = self.get_store()?;
        let rows = store.get_all(namespace)?;
        Ok(rows
            .into_iter()
            .map(|(k, v)| KeyValueEntry { key: k, value: v })
            .collect())
    }

    pub fn get_key(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        if !self.db_path.exists() {
            return Ok(None);
        }
        let store = self.get_store()?;
        let val = store.get(namespace, key)?;
        Ok(val)
    }

    pub fn set_key(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        let store = self.get_store()?;
        store.set(namespace, key, value)?;
        Ok(())
    }

    pub fn delete_key(&self, namespace: &str, key: &str) -> Result<()> {
        if !self.db_path.exists() {
            return Ok(());
        }
        let store = self.get_store()?;
        store.remove(namespace, key)?;
        Ok(())
    }

    pub fn clear_table(&self, namespace: &str) -> Result<usize> {
        if !self.db_path.exists() {
            return Ok(0);
        }
        let store = self.get_store()?;
        let count = store.clear_namespace(namespace)?;
        Ok(count)
    }

    pub fn reset_db(&self) -> Result<()> {
        let store = self.get_store()?;
        store.reset_database()?;
        Ok(())
    }

    pub fn backup_db(&self) -> Result<Vec<u8>> {
        let store = self.get_store()?;
        store.backup_bytes()
    }

    pub fn restore_db(&self, data: &[u8]) -> Result<()> {
        let store = self.get_store()?;
        store.restore_from_bytes(data)
    }

    pub fn telemetry(&self) -> Result<DbTelemetryStats> {
        let store = self.get_store()?;
        Ok(store.telemetry_stats())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn test_database_manager_operations() {
        let temp_dir = std::env::temp_dir().join(format!(
            "heimdall_db_mgr_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db_path = temp_dir.join("test_mgr.db");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mgr = DatabaseManager::new(&db_path);
        let summary_empty = mgr.summary().unwrap();
        assert_eq!(summary_empty.total_tables, 0);

        // Set keys in two tables
        mgr.set_key("users", "node1", r#"{"nickname":"Alpha"}"#).unwrap();
        mgr.set_key("users", "node2", r#"{"nickname":"Beta"}"#).unwrap();
        mgr.set_key("minidungeon", "hero", r#"{"hp":50}"#).unwrap();

        let summary = mgr.summary().unwrap();
        assert!(summary.exists);
        assert_eq!(summary.total_tables, 2);
        assert_eq!(summary.total_records, 3);

        let users = mgr.get_table_entries("users").unwrap();
        assert_eq!(users.len(), 2);

        let user1 = mgr.get_key("users", "node1").unwrap();
        assert_eq!(user1, Some(r#"{"nickname":"Alpha"}"#.to_string()));

        // Delete key
        mgr.delete_key("users", "node1").unwrap();
        let users_after_del = mgr.get_table_entries("users").unwrap();
        assert_eq!(users_after_del.len(), 1);

        // Clear table
        let cleared = mgr.clear_table("users").unwrap();
        assert_eq!(cleared, 1);
        let users_empty = mgr.get_table_entries("users").unwrap();
        assert_eq!(users_empty.len(), 0);

        // Backup
        let backup = mgr.backup_db().unwrap();
        assert!(!backup.is_empty());

        // Reset
        mgr.reset_db().unwrap();
        let summary_reset = mgr.summary().unwrap();
        assert_eq!(summary_reset.total_records, 0);

        // Restore
        mgr.restore_db(&backup).unwrap();
        let summary_restored = mgr.summary().unwrap();
        assert_eq!(summary_restored.total_records, 1);

        let tel = mgr.telemetry().unwrap();
        assert!(tel.total_queries > 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
