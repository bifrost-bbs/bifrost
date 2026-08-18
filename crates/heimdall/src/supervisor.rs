//! Master process supervisor for managing bifrost-bbs, bifrost-client, crawler, and tuning binaries.

use anyhow::{Context, Result};
use crate::logs::LogBuffer;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Stopped,
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub name: String,
    pub state: ProcessState,
    pub pid: Option<u32>,
    pub uptime_secs: u64,
    pub restart_count: u32,
    pub last_error: Option<String>,
}

pub struct ManagedProcess {
    pub name: String,
    pub child: Option<Child>,
    pub state: ProcessState,
    pub started_at: Option<Instant>,
    pub restart_count: u32,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct Supervisor {
    workspace_root: PathBuf,
    log_buffer: Arc<LogBuffer>,
    bbs_process: Arc<RwLock<ManagedProcess>>,
    crawler_process: Arc<RwLock<ManagedProcess>>,
    tuning_process: Arc<RwLock<ManagedProcess>>,
}

impl Supervisor {
    pub fn new(workspace_root: impl AsRef<Path>, log_buffer: Arc<LogBuffer>) -> Self {
        let root = workspace_root.as_ref().to_path_buf();
        Self {
            workspace_root: root,
            log_buffer,
            bbs_process: Arc::new(RwLock::new(ManagedProcess {
                name: "bifrost-bbs".to_string(),
                child: None,
                state: ProcessState::Stopped,
                started_at: None,
                restart_count: 0,
                last_error: None,
            })),
            crawler_process: Arc::new(RwLock::new(ManagedProcess {
                name: "bifrost-crawler".to_string(),
                child: None,
                state: ProcessState::Stopped,
                started_at: None,
                restart_count: 0,
                last_error: None,
            })),
            tuning_process: Arc::new(RwLock::new(ManagedProcess {
                name: "bifrost-tuning".to_string(),
                child: None,
                state: ProcessState::Stopped,
                started_at: None,
                restart_count: 0,
                last_error: None,
            })),
        }
    }

    pub fn get_workspace_root(&self) -> PathBuf {
        self.workspace_root.clone()
    }

    pub async fn get_all_status(&self) -> Vec<ProcessInfo> {
        vec![
            self.get_process_info(&self.bbs_process).await,
            self.get_process_info(&self.crawler_process).await,
            self.get_process_info(&self.tuning_process).await,
        ]
    }

    pub async fn get_bbs_status(&self) -> ProcessInfo {
        self.get_process_info(&self.bbs_process).await
    }

    async fn get_process_info(&self, proc_lock: &Arc<RwLock<ManagedProcess>>) -> ProcessInfo {
        let mut proc = proc_lock.write().await;
        
        // Check if child has exited
        if let Some(ref mut child) = proc.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    proc.child = None;
                    if status.success() {
                        proc.state = ProcessState::Stopped;
                    } else {
                        proc.state = ProcessState::Failed;
                        proc.last_error = Some(format!("Exited with code: {:?}", status.code()));
                    }
                }
                Ok(None) => {
                    proc.state = ProcessState::Running;
                }
                Err(e) => {
                    proc.state = ProcessState::Failed;
                    proc.last_error = Some(e.to_string());
                }
            }
        } else if proc.state == ProcessState::Running {
            proc.state = ProcessState::Stopped;
        }

        let uptime_secs = proc.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let pid = proc.child.as_ref().and_then(|c| c.id());

        ProcessInfo {
            name: proc.name.clone(),
            state: proc.state,
            pid,
            uptime_secs,
            restart_count: proc.restart_count,
            last_error: proc.last_error.clone(),
        }
    }

    pub async fn start_bbs(&self, config_path: Option<&str>, capture_dir: Option<&str>) -> Result<()> {
        let mut proc = self.bbs_process.write().await;
        if proc.state == ProcessState::Running && proc.child.is_some() {
            log::info!("bifrost-bbs is already running");
            return Ok(());
        }

        proc.state = ProcessState::Starting;
        self.log_buffer.push("heimdall", "INFO", "Starting bifrost-bbs daemon...");

        let bin_path = self.find_or_build_binary("bifrost-bbs").await?;
        let cfg_file = config_path.unwrap_or("config.toml");
        let cap_dir = capture_dir.unwrap_or("captured_packets");

        let mut cmd = Command::new(&bin_path);
        cmd.current_dir(&self.workspace_root);
        cmd.arg("--config").arg(cfg_file);
        cmd.arg("--mock");
        cmd.arg("--capture").arg(cap_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| format!("Failed to spawn {:?}", bin_path))?;
        
        let pid = child.id().unwrap_or(0);
        self.log_buffer.push("heimdall", "INFO", &format!("bifrost-bbs spawned (PID: {})", pid));

        // Pipe stdout to log buffer
        if let Some(stdout) = child.stdout.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_bbs", &line);
                }
            });
        }

        // Pipe stderr to log buffer
        if let Some(stderr) = child.stderr.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_bbs", &line);
                }
            });
        }

        proc.child = Some(child);
        proc.state = ProcessState::Running;
        proc.started_at = Some(Instant::now());
        proc.last_error = None;

        Ok(())
    }

    pub async fn stop_bbs(&self) -> Result<()> {
        let mut proc = self.bbs_process.write().await;
        if let Some(mut child) = proc.child.take() {
            self.log_buffer.push("heimdall", "INFO", "Stopping bifrost-bbs daemon...");
            let _ = child.kill().await;
            let _ = child.wait().await;
            self.log_buffer.push("heimdall", "INFO", "bifrost-bbs stopped.");
        }
        proc.state = ProcessState::Stopped;
        proc.started_at = None;
        Ok(())
    }

    pub async fn restart_bbs(&self, config_path: Option<&str>, capture_dir: Option<&str>) -> Result<()> {
        self.stop_bbs().await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        {
            let mut proc = self.bbs_process.write().await;
            proc.restart_count += 1;
        }
        self.start_bbs(config_path, capture_dir).await?;
        Ok(())
    }

    pub async fn start_crawler(&self, steps: usize, delay_ms: u64) -> Result<()> {
        let mut proc = self.crawler_process.write().await;
        if let Some(mut existing) = proc.child.take() {
            let _ = existing.kill().await;
        }

        self.log_buffer.push("heimdall", "INFO", &format!("Launching automated crawler (steps: {}, delay: {}ms)...", steps, delay_ms));

        let bin_path = self.find_or_build_binary("bifrost-client").await?;
        let mut cmd = Command::new(&bin_path);
        cmd.current_dir(&self.workspace_root);
        cmd.arg("--crawl");
        cmd.arg("--crawl-steps").arg(steps.to_string());
        cmd.arg("--crawl-delay").arg(delay_ms.to_string());
        cmd.arg("--headless");
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| format!("Failed to spawn {:?}", bin_path))?;

        if let Some(stdout) = child.stdout.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_client", &line);
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_client", &line);
                }
            });
        }

        proc.child = Some(child);
        proc.state = ProcessState::Running;
        proc.started_at = Some(Instant::now());

        Ok(())
    }

    pub async fn run_tuning(&self, subcmd: &str, extra_args: &[&str]) -> Result<()> {
        let mut proc = self.tuning_process.write().await;
        if let Some(mut existing) = proc.child.take() {
            let _ = existing.kill().await;
        }

        self.log_buffer.push("heimdall", "INFO", &format!("Running bifrost-tuning {} {:?}...", subcmd, extra_args));

        let bin_path = self.find_or_build_binary("bifrost-tuning").await?;
        let mut cmd = Command::new(&bin_path);
        cmd.current_dir(&self.workspace_root);
        cmd.arg(subcmd);
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().with_context(|| format!("Failed to spawn {:?}", bin_path))?;

        if let Some(stdout) = child.stdout.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_tuning", &line);
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let log_buf = self.log_buffer.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    log_buf.push_raw_line("bifrost_tuning", &line);
                }
            });
        }

        proc.child = Some(child);
        proc.state = ProcessState::Running;
        proc.started_at = Some(Instant::now());

        Ok(())
    }

    async fn find_or_build_binary(&self, bin_name: &str) -> Result<PathBuf> {
        let debug_bin = self.workspace_root.join("target/debug").join(bin_name);
        let release_bin = self.workspace_root.join("target/release").join(bin_name);

        if debug_bin.exists() {
            return Ok(debug_bin);
        }
        if release_bin.exists() {
            return Ok(release_bin);
        }

        // Try compiling if missing
        self.log_buffer.push("heimdall", "INFO", &format!("Binary {} not found. Compiling via cargo...", bin_name));
        let status = Command::new("cargo")
            .current_dir(&self.workspace_root)
            .args(["build", "--bin", bin_name])
            .status()
            .await
            .with_context(|| format!("Failed to compile binary {}", bin_name))?;

        if status.success() && debug_bin.exists() {
            Ok(debug_bin)
        } else {
            anyhow::bail!("Failed to find or build binary '{}'", bin_name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_supervisor_initial_state() {
        let temp_dir = std::env::temp_dir();
        let log_buf = Arc::new(LogBuffer::default());
        let sup = Supervisor::new(&temp_dir, log_buf);

        let status = sup.get_all_status().await;
        assert_eq!(status.len(), 3);
        assert_eq!(status[0].name, "bifrost-bbs");
        assert_eq!(status[0].state, ProcessState::Stopped);
    }
}
