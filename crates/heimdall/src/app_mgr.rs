//! Application manager for discovering, inspecting, editing, installing, and updating Lua BBS apps.

use anyhow::{Context, Result};
use bifrost_bbs::AppManifest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/bifrost-bbs/app-catalog/main/catalog.json";

pub const EMBEDDED_CATALOG_JSON: &str = r#"{
  "catalog_version": 1,
  "updated_at": "2026-08-24T05:25:00Z",
  "apps": [
    {
      "id": "minidungeon",
      "name": "Mini Dungeon",
      "author": "Bifrost Contributors",
      "description": "Turn-based rogue-like dungeon crawler door game with monsters, traps, loot, and persistent leveling.",
      "category": "games",
      "repository": "https://github.com/bifrost-bbs/app-minidungeon",
      "latest_version": "0.1.0",
      "latest_tag": "v0.1.0",
      "icon": "⚔️",
      "releases": [
        {
          "version": "0.1.0",
          "tag": "v0.1.0",
          "published_at": "2026-08-24T05:24:00Z",
          "min_bifrost_version": "0.1.0",
          "tarball_url": "https://github.com/bifrost-bbs/app-minidungeon/archive/refs/tags/v0.1.0.tar.gz",
          "changelog": "Initial standalone v0.1.0 beta release for Bifrost MeshBBS."
        }
      ]
    },
    {
      "id": "marketplace",
      "name": "Marketplace",
      "author": "Bifrost Contributors",
      "description": "Decentralized classified listings, barter board, and mesh auctions with direct operator messaging.",
      "category": "marketplace",
      "repository": "https://github.com/bifrost-bbs/app-marketplace",
      "latest_version": "0.1.0",
      "latest_tag": "v0.1.0",
      "icon": "🏷️",
      "releases": [
        {
          "version": "0.1.0",
          "tag": "v0.1.0",
          "published_at": "2026-08-24T05:26:00Z",
          "min_bifrost_version": "0.1.0",
          "tarball_url": "https://github.com/bifrost-bbs/app-marketplace/archive/refs/tags/v0.1.0.tar.gz",
          "changelog": "Initial standalone v0.1.0 beta release for Bifrost MeshBBS."
        }
      ]
    },
    {
      "id": "weather",
      "name": "Weather Forecast",
      "author": "Bifrost Contributors",
      "description": "Fetches local weather forecasts and telemetry using client node geolocation.",
      "category": "utilities",
      "repository": "https://github.com/bifrost-bbs/app-weather",
      "latest_version": "0.1.0",
      "latest_tag": "v0.1.0",
      "icon": "⛅",
      "releases": [
        {
          "version": "0.1.0",
          "tag": "v0.1.0",
          "published_at": "2026-08-24T05:26:00Z",
          "min_bifrost_version": "0.1.0",
          "tarball_url": "https://github.com/bifrost-bbs/app-weather/archive/refs/tags/v0.1.0.tar.gz",
          "changelog": "Initial standalone v0.1.0 beta release for Bifrost MeshBBS."
        }
      ]
    },
    {
      "id": "voidtrader",
      "name": "Void Trader",
      "author": "Bifrost Contributors",
      "description": "Space merchant and interstellar sector trading strategy door game inspired by classic TradeWars 2002.",
      "category": "games",
      "repository": "https://github.com/bifrost-bbs/app-voidtrader",
      "latest_version": "0.1.0",
      "latest_tag": "v0.1.0",
      "icon": "🚀",
      "releases": [
        {
          "version": "0.1.0",
          "tag": "v0.1.0",
          "published_at": "2026-08-24T05:26:00Z",
          "min_bifrost_version": "0.1.0",
          "tarball_url": "https://github.com/bifrost-bbs/app-voidtrader/archive/refs/tags/v0.1.0.tar.gz",
          "changelog": "Initial standalone v0.1.0 beta release for Bifrost MeshBBS."
        }
      ]
    }
  ]
}"#;

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
    pub is_builtin: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRelease {
    pub version: String,
    pub tag: String,
    pub published_at: String,
    #[serde(default)]
    pub min_bifrost_version: Option<String>,
    pub tarball_url: String,
    #[serde(default)]
    pub changelog: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogApp {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub category: String,
    pub repository: String,
    pub latest_version: String,
    pub latest_tag: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub releases: Vec<CatalogRelease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppCatalogIndex {
    pub catalog_version: u32,
    pub updated_at: String,
    pub apps: Vec<CatalogApp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogAppStatus {
    #[serde(flatten)]
    pub catalog_app: CatalogApp,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub update_available: bool,
    pub enabled: bool,
    pub is_builtin: bool,
}

#[derive(Debug)]
pub struct AppManager {
    apps_dir: PathBuf,
    catalog_cache: Mutex<Option<(Instant, AppCatalogIndex)>>,
}

impl AppManager {
    pub fn new(apps_dir: impl AsRef<Path>) -> Self {
        Self {
            apps_dir: apps_dir.as_ref().to_path_buf(),
            catalog_cache: Mutex::new(None),
        }
    }

    pub fn is_builtin(app_id: &str) -> bool {
        matches!(app_id, "admin" | "profile" | "messages")
    }

    pub fn validate_app_id(app_id: &str) -> Result<()> {
        if app_id.is_empty()
            || app_id.len() > 64
            || !app_id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            anyhow::bail!(
                "Invalid app ID '{}': must be 1-64 lowercase alphanumeric or underscore characters",
                app_id
            );
        }
        Ok(())
    }

    pub fn list_apps(&self, enabled_list: &[String], main_app: &str) -> Vec<AppInfo> {
        let mut apps = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&self.apps_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let app_id = entry.file_name().to_string_lossy().to_string();
                    if app_id.starts_with('.') {
                        continue;
                    }
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
                                if let Some(v) = manifest.app.version {
                                    version = v;
                                }
                                if let Some(d) = manifest.app.description {
                                    description = d;
                                }
                                if let Some(a) = manifest.app.author {
                                    author = a;
                                }
                                if let Some(ep) = manifest.app.entry_point {
                                    entry_point = ep;
                                }
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
                    let is_builtin = Self::is_builtin(&app_id);

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
                        is_builtin,
                    });
                }
            }
        }

        apps.sort_by(|a, b| a.id.cmp(&b.id));
        apps
    }

    pub fn get_app_detail(
        &self,
        app_id: &str,
        enabled_list: &[String],
        main_app: &str,
    ) -> Result<AppDetail> {
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
            if let Some(v) = manifest.app.version {
                version = v;
            }
            if let Some(d) = manifest.app.description {
                description = d;
            }
            if let Some(a) = manifest.app.author {
                author = a;
            }
            if let Some(ep) = manifest.app.entry_point {
                entry_point = ep;
            }
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
        let is_builtin = Self::is_builtin(app_id);

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
            is_builtin,
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

    fn collect_files_recursive(
        &self,
        root: &Path,
        current: &Path,
        out: &mut Vec<AppFileEntry>,
    ) -> Result<()> {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dir = path.is_dir();
                let rel_path = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                let name = entry.file_name().to_string_lossy().to_string();
                let size_bytes = if is_dir {
                    0
                } else {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                };

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

    // --- APP CATALOG & APP STORE METHODS ---

    /// Fetches the remote catalog index, caching for 5 minutes with embedded fallback.
    pub async fn fetch_catalog(
        &self,
        custom_url: Option<&str>,
        force_refresh: bool,
    ) -> Result<AppCatalogIndex> {
        if !force_refresh {
            if let Ok(guard) = self.catalog_cache.lock() {
                if let Some((fetched_at, ref catalog)) = *guard {
                    if fetched_at.elapsed() < Duration::from_secs(300) {
                        return Ok(catalog.clone());
                    }
                }
            }
        }

        let url = custom_url.unwrap_or(DEFAULT_CATALOG_URL);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Heimdall-Supervisor/0.1.0 (Bifrost)")
            .build();

        let fetched = match client {
            Ok(c) => match c.get(url).send().await {
                Ok(resp) if resp.status().is_success() => resp.json::<AppCatalogIndex>().await.ok(),
                _ => None,
            },
            _ => None,
        };

        let catalog = match fetched {
            Some(cat) => cat,
            None => {
                log::warn!(
                    "Could not fetch live catalog from {}; using embedded catalog fallback",
                    url
                );
                serde_json::from_str::<AppCatalogIndex>(EMBEDDED_CATALOG_JSON)
                    .with_context(|| "Failed to parse embedded catalog JSON")?
            }
        };

        if let Ok(mut guard) = self.catalog_cache.lock() {
            *guard = Some((Instant::now(), catalog.clone()));
        }

        Ok(catalog)
    }

    /// Merges the catalog with local installation and activation status.
    pub fn get_catalog_status(
        &self,
        catalog: &AppCatalogIndex,
        enabled_list: &[String],
        main_app: &str,
    ) -> Vec<CatalogAppStatus> {
        let installed_apps = self.list_apps(enabled_list, main_app);

        let mut statuses = Vec::new();
        for cat_app in &catalog.apps {
            let local = installed_apps.iter().find(|a| a.id == cat_app.id);
            let installed = local.is_some();
            let installed_version = local.map(|a| a.version.clone());
            let enabled = local.map(|a| a.enabled).unwrap_or(false);
            let is_builtin = Self::is_builtin(&cat_app.id);

            let update_available = match &installed_version {
                Some(v) => v != &cat_app.latest_version,
                None => false,
            };

            statuses.push(CatalogAppStatus {
                catalog_app: cat_app.clone(),
                installed,
                installed_version,
                update_available,
                enabled,
                is_builtin,
            });
        }

        statuses
    }

    /// Installs an app archive from tarball bytes.
    pub fn install_app_from_tarball_bytes(&self, app_id: &str, tarball_bytes: &[u8]) -> Result<()> {
        Self::validate_app_id(app_id)?;
        if Self::is_builtin(app_id) {
            anyhow::bail!("Cannot overwrite core built-in application '{}'", app_id);
        }

        let target_dir = self.apps_dir.join(app_id);
        let temp_extract = self.apps_dir.join(format!(
            ".tmp_install_{}_{}",
            app_id,
            Instant::now().elapsed().as_nanos()
        ));
        if temp_extract.exists() {
            let _ = std::fs::remove_dir_all(&temp_extract);
        }
        std::fs::create_dir_all(&temp_extract)?;

        let tar = flate2::read::GzDecoder::new(tarball_bytes);
        let mut archive = tar::Archive::new(tar);
        archive
            .unpack(&temp_extract)
            .with_context(|| "Failed to unpack tarball")?;

        // Find the root directory inside temp_extract
        let mut subdirs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&temp_extract) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    subdirs.push(entry.path());
                }
            }
        }

        let source_dir = if subdirs.len() == 1 {
            subdirs[0].clone()
        } else {
            temp_extract.clone()
        };

        if target_dir.exists() {
            let _ = std::fs::remove_dir_all(&target_dir);
        }
        std::fs::create_dir_all(&target_dir)?;

        Self::copy_dir_recursive(&source_dir, &target_dir)?;
        let _ = std::fs::remove_dir_all(&temp_extract);

        let manifest_path = target_dir.join("manifest.toml");
        if !manifest_path.exists() {
            anyhow::bail!("Downloaded archive does not contain a valid manifest.toml at root");
        }

        log::info!(
            "Successfully installed app '{}' into {:?}",
            app_id,
            target_dir
        );
        Ok(())
    }

    /// Downloads and installs an app from the catalog.
    pub async fn install_catalog_app(
        &self,
        app_id: &str,
        tag: Option<&str>,
        catalog_index: &AppCatalogIndex,
    ) -> Result<()> {
        Self::validate_app_id(app_id)?;
        if Self::is_builtin(app_id) {
            anyhow::bail!("Cannot overwrite core built-in application '{}'", app_id);
        }

        let cat_app = catalog_index
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .ok_or_else(|| anyhow::anyhow!("App '{}' not found in catalog", app_id))?;

        let release = if let Some(target_tag) = tag {
            cat_app
                .releases
                .iter()
                .find(|r| r.tag == target_tag)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Release tag '{}' not found for app '{}'",
                        target_tag,
                        app_id
                    )
                })?
        } else {
            cat_app
                .releases
                .iter()
                .find(|r| r.tag == cat_app.latest_tag)
                .or_else(|| cat_app.releases.first())
                .ok_or_else(|| anyhow::anyhow!("No releases available for app '{}'", app_id))?
        };

        // Try downloading tarball via reqwest
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Heimdall-AppStore/0.1.0 (Bifrost)")
            .build()?;

        let resp = client.get(&release.tarball_url).send().await;
        if let Ok(res) = resp {
            if res.status().is_success() {
                if let Ok(bytes) = res.bytes().await {
                    if self.install_app_from_tarball_bytes(app_id, &bytes).is_ok() {
                        return Ok(());
                    }
                }
            }
        }

        // Fallback: git clone if git is available
        let target_dir = self.apps_dir.join(app_id);
        if target_dir.exists() {
            let _ = std::fs::remove_dir_all(&target_dir);
        }
        let tag_to_clone = tag.unwrap_or(&cat_app.latest_tag);
        let status = tokio::process::Command::new("git")
            .args(&[
                "clone",
                "--depth",
                "1",
                "--branch",
                tag_to_clone,
                &cat_app.repository,
                target_dir.to_str().unwrap(),
            ])
            .status()
            .await;

        if let Ok(st) = status {
            if st.success() && target_dir.join("manifest.toml").exists() {
                log::info!(
                    "Successfully cloned app '{}' via git into {:?}",
                    app_id,
                    target_dir
                );
                return Ok(());
            }
        }

        anyhow::bail!(
            "Failed to download and install app '{}' from {}",
            app_id,
            release.tarball_url
        );
    }

    /// Uninstalls an installed application.
    pub fn uninstall_app(&self, app_id: &str) -> Result<()> {
        Self::validate_app_id(app_id)?;
        if Self::is_builtin(app_id) {
            anyhow::bail!("Cannot uninstall core built-in application '{}'", app_id);
        }
        let app_dir = self.apps_dir.join(app_id);
        if !app_dir.exists() {
            anyhow::bail!("App '{}' is not installed", app_id);
        }
        std::fs::remove_dir_all(&app_dir)
            .with_context(|| format!("Failed to remove app directory {:?}", app_dir))?;
        log::info!("Uninstalled app '{}' from {:?}", app_id, app_dir);
        Ok(())
    }

    fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
        if !dst.exists() {
            std::fs::create_dir_all(dst)?;
        }
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let dest_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                Self::copy_dir_recursive(&entry.path(), &dest_path)?;
            } else {
                std::fs::copy(entry.path(), dest_path)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_app_manager_discovery_and_detail() {
        let temp_dir = std::env::temp_dir().join(format!(
            "heimdall_app_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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

        let detail = mgr
            .get_app_detail("test_app", &["test_app".to_string()], "test_app")
            .unwrap();
        assert_eq!(detail.info.name, "Test Application");
        assert_eq!(detail.main_lua, "-- Lua main");
        assert_eq!(detail.assets.len(), 1);

        mgr.save_app_file("test_app", "helper.lua", "-- helper code")
            .unwrap();
        assert!(app_dir.join("helper.lua").exists());

        let files = mgr.list_app_files("test_app").unwrap();
        assert!(files.iter().any(|f| f.path == "main.lua"));
        assert!(files.iter().any(|f| f.path == "manifest.toml"));
        assert!(files.iter().any(|f| f.path == "helper.lua"));
        assert!(files.iter().any(|f| f.path.starts_with("assets")));

        let helper_text = mgr.read_app_file("test_app", "helper.lua").unwrap();
        assert_eq!(helper_text, "-- helper code");

        // Test catalog fetching
        let catalog = mgr.fetch_catalog(None, false).await.unwrap();
        assert!(catalog.apps.len() >= 4);

        let statuses = mgr.get_catalog_status(&catalog, &["test_app".to_string()], "test_app");
        assert_eq!(statuses.len(), catalog.apps.len());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
