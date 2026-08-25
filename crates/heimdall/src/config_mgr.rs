//! Configuration manager for inspecting, editing, and validating `config.toml`.

use anyhow::{Context, Result};
use bifrost_bbs::{default_config, AppConfig};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub raw_toml: String,
    pub parsed: AppConfig,
    pub path: String,
}

#[derive(Debug)]
pub struct ConfigManager {
    config_path: PathBuf,
    current_config: Mutex<AppConfig>,
}

impl ConfigManager {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let config_path = path.as_ref().to_path_buf();
        let loaded = Self::load_from_path(&config_path).unwrap_or_else(|e| {
            log::warn!(
                "Failed to load config from {:?}: {}. Using defaults.",
                config_path,
                e
            );
            default_config()
        });

        Self {
            config_path,
            current_config: Mutex::new(loaded),
        }
    }

    pub fn get_config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    pub fn get_config(&self) -> AppConfig {
        self.current_config.lock().unwrap().clone()
    }

    pub fn get_raw_toml(&self) -> Result<String> {
        if self.config_path.exists() {
            std::fs::read_to_string(&self.config_path)
                .with_context(|| format!("Failed to read {:?}", self.config_path))
        } else {
            toml::to_string_pretty(&*self.current_config.lock().unwrap())
                .context("Failed to serialize default config to TOML")
        }
    }

    pub fn get_response(&self) -> Result<ConfigResponse> {
        let raw_toml = self.get_raw_toml()?;
        let parsed = self.get_config();
        let path = self.config_path.to_string_lossy().to_string();
        Ok(ConfigResponse {
            raw_toml,
            parsed,
            path,
        })
    }

    pub fn save_raw_toml(&self, raw_toml: &str) -> Result<AppConfig> {
        // Validate TOML parses cleanly into AppConfig
        let parsed: AppConfig = toml::from_str(raw_toml)
            .with_context(|| "Invalid TOML syntax or schema for AppConfig")?;

        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }

        std::fs::write(&self.config_path, raw_toml)
            .with_context(|| format!("Failed to write config file to {:?}", self.config_path))?;

        *self.current_config.lock().unwrap() = parsed.clone();
        log::info!(
            "Successfully saved and reloaded config from {:?}",
            self.config_path
        );

        Ok(parsed)
    }

    pub fn save_config(&self, new_config: AppConfig) -> Result<()> {
        let toml_str = toml::to_string_pretty(&new_config)
            .context("Failed to serialize AppConfig to TOML string")?;
        self.save_raw_toml(&toml_str)?;
        Ok(())
    }

    fn load_from_path(path: &Path) -> Result<AppConfig> {
        if !path.exists() {
            anyhow::bail!("Config file {:?} does not exist", path);
        }
        let content = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_manager_load_and_save() {
        let temp_dir = std::env::temp_dir().join(format!(
            "heimdall_cfg_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let cfg_path = temp_dir.join("config.toml");

        let manager = ConfigManager::new(&cfg_path);
        let resp = manager.get_response().unwrap();
        assert!(!resp.raw_toml.is_empty());

        let raw_edit = r#"
        log_level = "debug"

        [rate_limiter]
        max_packets_per_minute = 60
        max_burst_packets = 5
        inter_packet_guard_ms = 300
        max_duty_cycle_percent = 1.0
        duty_cycle_window_secs = 3600

        [asset_broadcaster]
        enable_on_demand_broadcast = true
        max_asset_broadcast_duty_cycle = 0.20

        [form_colors]
        field_fg = 14
        field_bg = 1
        submit_fg = 15
        submit_bg = 2

        admin_nodes = ["11223344556677889900aabbccddeeff"]

        [apps]
        main_app = "main_menu"
        enabled = ["main_menu", "marketplace"]

        [packet_capture]
        enabled = true
        directory = "captured_packets"
        "#;

        let saved = manager.save_raw_toml(raw_edit).unwrap();
        assert_eq!(saved.log_level, "debug");
        assert_eq!(saved.rate_limiter.max_packets_per_minute, 60);
        assert_eq!(saved.apps.enabled.len(), 2);
        assert!(saved.packet_capture.enabled);

        let reread = ConfigManager::new(&cfg_path);
        assert_eq!(reread.get_config().log_level, "debug");
        assert_eq!(reread.get_config().rate_limiter.max_burst_packets, 5);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
