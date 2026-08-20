//! Application manager for discovering, inspecting, and editing Lua BBS apps.

use anyhow::{Context, Result};
use bifrost_bbs::AppManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub entry_point: String,
    pub enabled: bool,
    pub is_main: bool,
    pub asset_count: usize,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDetail {
    pub info: AppInfo,
    pub manifest_raw: String,
    pub main_lua: String,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppFileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub struct AppManager {
    apps_dir: PathBuf,
}

impl AppManager {
    pub fn new(apps_dir: impl AsRef<Path>) -> Self {
        Self {
            apps_dir: apps_dir.as_ref().to_path_buf(),
        }
    }

    pub fn list_apps(&self, enabled_list: &[String], main_app: &str) -> Vec<AppInfo> {
        let mut apps = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.apps_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let app_id = entry.file_name().to_string_lossy().to_string();
                    let manifest_path = path.join("manifest.toml");
                    let mut name = app_id.clone();
                    let mut version = "0.1.0".to_string();
                    let mut description = String::new();
                    let mut author = "Unknown".to_string();
                    let mut entry_point = "main.lua".to_string();
                    let mut asset_count = 0;

                    if manifest_path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                            if let Ok(manifest) = toml::from_str::<AppManifest>(&content) {
                                name = manifest.app.name;
                                if let Some(v) = manifest.app.version { version = v; }
                                if let Some(d) = manifest.app.description { description = d; }
                                if let Some(a) = manifest.app.author { author = a; }
                                if let Some(ep) = manifest.app.entry_point { entry_point = ep; }
                                asset_count = manifest.assets.len();
                            }
                        }
                    }

                    // Count assets in assets/ dir if exists
                    let assets_dir = path.join("assets");
                    if assets_dir.exists() && assets_dir.is_dir() {
                        if let Ok(asset_entries) = std::fs::read_dir(&assets_dir) {
                            let disk_asset_count = asset_entries.flatten().count();
                            if disk_asset_count > asset_count {
                                asset_count = disk_asset_count;
                            }
                        }
                    }

                    let enabled = enabled_list.iter().any(|e| e == &app_id);
                    let is_main = app_id == main_app;

                    apps.push(AppInfo {
                        id: app_id,
                        name,
                        version,
                        description,
                        author,
                        entry_point,
                        enabled,
                        is_main,
                        asset_count,
                        path: path.to_string_lossy().to_string(),
                    });
                }
            }
        }

        apps.sort_by(|a, b| a.id.cmp(&b.id));
        apps
    }

    pub fn get_app_detail(&self, app_id: &str, enabled_list: &[String], main_app: &str) -> Result<AppDetail> {
        let app_dir = self.apps_dir.join(app_id);
        if !app_dir.exists() || !app_dir.is_dir() {
            anyhow::bail!("App '{}' not found in {:?}", app_id, self.apps_dir);
        }

        let manifest_path = app_dir.join("manifest.toml");
        let manifest_raw = if manifest_path.exists() {
            std::fs::read_to_string(&manifest_path).unwrap_or_default()
        } else {
            String::new()
        };

        let mut name = app_id.to_string();
        let mut version = "0.1.0".to_string();
        let mut description = String::new();
        let mut author = "Unknown".to_string();
        let mut entry_point = "main.lua".to_string();
        let mut asset_count = 0;

        if let Ok(manifest) = toml::from_str::<AppManifest>(&manifest_raw) {
            name = manifest.app.name;
            if let Some(v) = manifest.app.version { version = v; }
            if let Some(d) = manifest.app.description { description = d; }
            if let Some(a) = manifest.app.author { author = a; }
            if let Some(ep) = manifest.app.entry_point { entry_point = ep; }
            asset_count = manifest.assets.len();
        }

        let main_lua_path = app_dir.join(&entry_point);
        let main_lua = if main_lua_path.exists() {
            std::fs::read_to_string(&main_lua_path).unwrap_or_default()
        } else {
            String::new()
        };

        let mut assets = Vec::new();
        let assets_dir = app_dir.join("assets");
        if assets_dir.exists() && assets_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    assets.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }

        let enabled = enabled_list.iter().any(|e| e == app_id);
        let is_main = app_id == main_app;

        let info = AppInfo {
            id: app_id.to_string(),
            name,
            version,
            description,
            author,
            entry_point,
            enabled,
            is_main,
            asset_count: asset_count.max(assets.len()),
            path: app_dir.to_string_lossy().to_string(),
        };

        Ok(AppDetail {
            info,
            manifest_raw,
            main_lua,
            assets,
        })
    }

    pub fn list_app_files(&self, app_id: &str) -> Result<Vec<AppFileEntry>> {
        let app_dir = self.apps_dir.join(app_id);
        if !app_dir.exists() || !app_dir.is_dir() {
            anyhow::bail!("App '{}' not found", app_id);
        }

        let mut files = Vec::new();
        self.collect_files_recursive(&app_dir, &app_dir, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn collect_files_recursive(&self, root: &Path, current: &Path, out: &mut Vec<AppFileEntry>) -> Result<()> {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dir = path.is_dir();
                let rel_path = path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                let name = entry.file_name().to_string_lossy().to_string();
                let size_bytes = if is_dir { 0 } else { entry.metadata().map(|m| m.len()).unwrap_or(0) };

                out.push(AppFileEntry {
                    path: rel_path,
                    name,
                    is_dir,
                    size_bytes,
                });

                if is_dir {
                    self.collect_files_recursive(root, &path, out)?;
                }
            }
        }
        Ok(())
    }

    pub fn read_app_file(&self, app_id: &str, relative_path: &str) -> Result<String> {
        let app_dir = self.apps_dir.join(app_id);
        if !app_dir.exists() {
            anyhow::bail!("App '{}' not found", app_id);
        }

        if relative_path.contains("..") || relative_path.starts_with('/') {
            anyhow::bail!("Invalid filename: path traversal not allowed");
        }

        let file_path = app_dir.join(relative_path);
        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read file {:?}", file_path))?;
        Ok(content)
    }

    pub fn save_app_file(&self, app_id: &str, filename: &str, content: &str) -> Result<()> {
        let app_dir = self.apps_dir.join(app_id);
        if !app_dir.exists() {
            std::fs::create_dir_all(&app_dir)?;
        }

        // Prevent path traversal
        if filename.contains("..") || filename.starts_with('/') {
            anyhow::bail!("Invalid filename: path traversal not allowed");
        }

        let file_path = app_dir.join(filename);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&file_path, content)
            .with_context(|| format!("Failed to write file {:?}", file_path))?;

        log::info!("Saved app file {:?}", file_path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_manager_discovery_and_detail() {
        let temp_dir = std::env::temp_dir().join(format!("heimdall_app_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let app_dir = temp_dir.join("test_app");
        std::fs::create_dir_all(app_dir.join("assets")).unwrap();

        let manifest = r#"
        [app]
        id = "test_app"
        name = "Test Application"
        version = "1.2.3"
        description = "A unit test app"
        author = "Heimdall"
        entry_point = "main.lua"
        "#;
        std::fs::write(app_dir.join("manifest.toml"), manifest).unwrap();
        std::fs::write(app_dir.join("main.lua"), "-- Lua main").unwrap();
        std::fs::write(app_dir.join("assets").join("logo.ans"), "ANSI LOGO").unwrap();

        let mgr = AppManager::new(&temp_dir);
        let list = mgr.list_apps(&["test_app".to_string()], "test_app");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "test_app");
        assert_eq!(list[0].name, "Test Application");
        assert_eq!(list[0].version, "1.2.3");
        assert!(list[0].enabled);
        assert!(list[0].is_main);

        let detail = mgr.get_app_detail("test_app", &["test_app".to_string()], "test_app").unwrap();
        assert_eq!(detail.info.name, "Test Application");
        assert_eq!(detail.main_lua, "-- Lua main");
        assert_eq!(detail.assets.len(), 1);

        mgr.save_app_file("test_app", "helper.lua", "-- helper code").unwrap();
        assert!(app_dir.join("helper.lua").exists());

        let files = mgr.list_app_files("test_app").unwrap();
        assert!(files.iter().any(|f| f.path == "main.lua"));
        assert!(files.iter().any(|f| f.path == "manifest.toml"));
        assert!(files.iter().any(|f| f.path == "helper.lua"));
        assert!(files.iter().any(|f| f.path.starts_with("assets")));

        let helper_text = mgr.read_app_file("test_app", "helper.lua").unwrap();
        assert_eq!(helper_text, "-- helper code");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
