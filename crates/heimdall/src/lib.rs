//! Heimdall: Master Supervisor Service & Retro Web Admin Dashboard for Bifrost MeshBBS.

pub mod app_mgr;
pub mod auth;
pub mod config_mgr;
pub mod db_mgr;
pub mod logs;
pub mod stats;
pub mod supervisor;
pub mod user_mgr;
pub mod web;
pub mod web_client;

use anyhow::Result;
use app_mgr::AppManager;
use auth::{AuthConfig, SessionManager};
use config_mgr::ConfigManager;
use db_mgr::DatabaseManager;
use logs::LogBuffer;
use stats::StatsManager;
use supervisor::Supervisor;
use std::path::PathBuf;
use std::sync::Arc;
use user_mgr::UserManager;
use web::{AppState, create_router};

#[derive(Debug, Clone)]
pub struct HeimdallConfig {
    pub port: u16,
    pub bind_addr: String,
    pub auth: AuthConfig,
    pub web_dir: Option<PathBuf>,
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub apps_dir: PathBuf,
    pub capture_dir: PathBuf,
    pub db_path: PathBuf,
    pub auto_start_bbs: bool,
    pub radio_port: String,
}

impl Default for HeimdallConfig {
    fn default() -> Self {
        let root = find_workspace_root();
        Self {
            port: 9324,
            bind_addr: "0.0.0.0".to_string(),
            auth: AuthConfig::default(),
            web_dir: None,
            config_path: root.join("config.toml"),
            apps_dir: root.join("apps"),
            capture_dir: root.join("captured_packets"),
            db_path: root.join("database.db"),
            workspace_root: root,
            auto_start_bbs: true,
            radio_port: "127.0.0.1:8088".to_string(),
        }
    }
}

pub struct HeimdallServer {
    config: HeimdallConfig,
    app_state: AppState,
}

impl HeimdallServer {
    pub fn new(config: HeimdallConfig) -> Self {
        let log_buffer = Arc::new(LogBuffer::new(5000));
        let supervisor = Supervisor::new(&config.workspace_root, log_buffer.clone());
        let config_mgr = Arc::new(ConfigManager::new(&config.config_path));
        let app_mgr = Arc::new(AppManager::new(&config.apps_dir));
        let stats_mgr = Arc::new(StatsManager::new(&config.capture_dir));
        let db_mgr = Arc::new(DatabaseManager::new(&config.db_path));
        let user_mgr = Arc::new(UserManager::new(&config.db_path));
        let session_mgr = Arc::new(SessionManager::new());

        let app_state = AppState {
            supervisor,
            config_mgr,
            app_mgr,
            stats_mgr,
            db_mgr,
            user_mgr,
            session_mgr,
            log_buffer,
            auth_config: config.auth.clone(),
            web_dir: config.web_dir.clone(),
            radio_port: config.radio_port.clone(),
        };

        Self { config, app_state }
    }

    pub async fn run(self) -> Result<()> {
        let addr = format!("{}:{}", self.config.bind_addr, self.config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        log::info!("╔════════════════════════════════════════════════════════════╗");
        log::info!("║  HEIMDALL SUPERVISOR RUNNING ON http://{}  ║", addr);
        log::info!("╚════════════════════════════════════════════════════════════╝");

        self.app_state
            .log_buffer
            .push("heimdall", "INFO", &format!("Heimdall Web Supervisor active on http://{}", addr));

        // Auto-start BBS daemon if enabled
        if self.config.auto_start_bbs {
            let cfg_str = self.config.config_path.to_string_lossy().to_string();
            let cap_str = self.config.capture_dir.to_string_lossy().to_string();
            if let Err(e) = self.app_state.supervisor.start_bbs(Some(&cfg_str), Some(&cap_str)).await {
                log::warn!("Auto-start BBS error: {}", e);
            }
        }

        let router = create_router(self.app_state);
        axum::serve(listener, router).await?;
        Ok(())
    }
}

pub fn find_workspace_root() -> PathBuf {
    // 1. Traverse upward from current working directory
    if let Ok(current) = std::env::current_dir() {
        let mut cur = current;
        for _ in 0..10 {
            if cur.join("Cargo.toml").exists() && cur.join("apps").exists() {
                return cur;
            }
            if let Some(parent) = cur.parent() {
                cur = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    // 2. Traverse upward from executable directory
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe;
        for _ in 0..10 {
            if cur.join("Cargo.toml").exists() && cur.join("apps").exists() {
                return cur;
            }
            if let Some(parent) = cur.parent() {
                cur = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    // 3. Traverse upward from CARGO_MANIFEST_DIR
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let mut cur = PathBuf::from(manifest_dir);
        for _ in 0..10 {
            if cur.join("Cargo.toml").exists() && cur.join("apps").exists() {
                return cur;
            }
            if let Some(parent) = cur.parent() {
                cur = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_workspace_root() {
        let root = find_workspace_root();
        assert!(root.exists());
    }

    #[test]
    fn test_heimdall_config_defaults() {
        let cfg = HeimdallConfig::default();
        assert_eq!(cfg.port, 9324);
        assert_eq!(cfg.bind_addr, "0.0.0.0");
        assert!(cfg.auto_start_bbs);
    }
}
