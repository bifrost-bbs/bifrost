//! Telemetry aggregator, metrics parser, and packet capture inspector.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetrySnapshot {
    pub active_sessions: usize,
    pub unique_users_24h: usize,
    pub total_packets_sent: u64,
    pub total_packets_received: u64,
    pub total_raw_bytes_sent: u64,
    pub total_compressed_bytes_sent: u64,
    pub total_raw_bytes_received: u64,
    pub total_compressed_bytes_received: u64,
    pub send_ppm_1h: f64,
    pub recv_ppm_1h: f64,
    pub send_ppm_24h: f64,
    pub recv_ppm_24h: f64,
    pub uptime_secs: u64,
    pub duty_cycle_percent: f64,
    pub compression_savings_percent: f64,
    pub captured_packets_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedPacketRow {
    pub timestamp: String,
    pub seq: u64,
    pub direction: String,
    pub category: String,
    pub opcode: String,
    pub flags: String,
    pub raw_bytes: usize,
    pub compressed_bytes: usize,
    pub savings_percent: f64,
    pub algorithm: String,
    pub duration_us: u64,
    pub raw_file: String,
    pub comp_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureAnalysisSummary {
    pub total_samples: usize,
    pub total_raw_bytes: usize,
    pub total_comp_bytes: usize,
    pub net_savings_percent: f64,
    pub tx_count: usize,
    pub rx_count: usize,
    pub avg_raw_bytes: f64,
    pub avg_comp_bytes: f64,
    pub avg_bytes_per_packet: f64,
    pub unique_users_count: usize,
    pub avg_bytes_per_packet_per_user: f64,
    pub avg_duration_us: f64,
    pub categories: std::collections::HashMap<String, usize>,
    pub algorithms: std::collections::HashMap<String, usize>,
}

#[derive(Debug)]
pub struct StatsManager {
    capture_dir: PathBuf,
}

impl StatsManager {
    pub fn new(capture_dir: impl AsRef<Path>) -> Self {
        Self {
            capture_dir: capture_dir.as_ref().to_path_buf(),
        }
    }

    pub fn get_capture_csv_path(&self) -> PathBuf {
        self.capture_dir.join("compression_log.csv")
    }

    pub fn get_captured_packets(&self, limit: Option<usize>, offset: Option<usize>) -> (Vec<CapturedPacketRow>, usize) {
        let csv_path = self.get_capture_csv_path();
        if !csv_path.exists() {
            return (Vec::new(), 0);
        }

        let content = match std::fs::read_to_string(&csv_path) {
            Ok(c) => c,
            Err(_) => return (Vec::new(), 0),
        };

        let mut rows = Vec::new();
        let lines: Vec<&str> = content.lines().collect();
        if lines.len() <= 1 {
            return (Vec::new(), 0);
        }

        for line in lines.iter().skip(1) {
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() >= 13 {
                let timestamp = cols[0].to_string();
                let seq = cols[1].parse().unwrap_or(0);
                let direction = cols[2].to_string();
                let category = cols[3].to_string();
                let opcode = cols[4].to_string();
                let flags = cols[5].to_string();
                let raw_bytes = cols[6].parse().unwrap_or(0);
                let compressed_bytes = cols[7].parse().unwrap_or(0);
                let savings_percent = cols[8].parse().unwrap_or(0.0);
                let algorithm = cols[9].to_string();
                let duration_us = cols[10].parse().unwrap_or(0);
                let raw_file = cols[11].to_string();
                let comp_file = cols[12].to_string();

                rows.push(CapturedPacketRow {
                    timestamp,
                    seq,
                    direction,
                    category,
                    opcode,
                    flags,
                    raw_bytes,
                    compressed_bytes,
                    savings_percent,
                    algorithm,
                    duration_us,
                    raw_file,
                    comp_file,
                });
            }
        }

        let total = rows.len();
        rows.reverse(); // Newest first

        let offset_val = offset.unwrap_or(0);
        let limit_val = limit.unwrap_or(100);

        let paged = rows
            .into_iter()
            .skip(offset_val)
            .take(limit_val)
            .collect();

        (paged, total)
    }

    pub fn get_capture_summary(&self) -> CaptureAnalysisSummary {
        let (rows, _) = self.get_captured_packets(None, None);
        if rows.is_empty() {
            return CaptureAnalysisSummary::default();
        }

        let mut summary = CaptureAnalysisSummary {
            total_samples: rows.len(),
            ..Default::default()
        };

        let mut total_duration = 0u64;

        for r in &rows {
            summary.total_raw_bytes += r.raw_bytes;
            summary.total_comp_bytes += r.compressed_bytes;
            total_duration += r.duration_us;

            if r.direction == "TX" {
                summary.tx_count += 1;
            } else {
                summary.rx_count += 1;
            }

            *summary.categories.entry(r.category.clone()).or_insert(0) += 1;
            *summary.algorithms.entry(r.algorithm.clone()).or_insert(0) += 1;
        }

        if summary.total_raw_bytes > 0 {
            let saved = summary.total_raw_bytes as f64 - summary.total_comp_bytes as f64;
            summary.net_savings_percent = (saved / summary.total_raw_bytes as f64) * 100.0;
            summary.avg_raw_bytes = summary.total_raw_bytes as f64 / rows.len() as f64;
            summary.avg_comp_bytes = summary.total_comp_bytes as f64 / rows.len() as f64;
            summary.avg_bytes_per_packet = summary.avg_comp_bytes;
            summary.unique_users_count = 1; // Baseline local/single user session if no user list
            summary.avg_bytes_per_packet_per_user = summary.avg_bytes_per_packet / summary.unique_users_count as f64;
            summary.avg_duration_us = total_duration as f64 / rows.len() as f64;
        }

        summary
    }

    pub fn read_sample_file(&self, relative_path: &str) -> Result<Vec<u8>> {
        if relative_path.contains("..") || relative_path.starts_with('/') {
            anyhow::bail!("Invalid path traversal");
        }
        let full_path = self.capture_dir.join(relative_path);
        let data = std::fs::read(&full_path)?;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_manager_csv_parsing_and_summary() {
        let temp_dir = std::env::temp_dir().join(format!("heimdall_stats_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let csv_content = r#"timestamp,seq,direction,category,opcode,flags,raw_bytes,compressed_bytes,savings_percent,algorithm,duration_us,raw_file,comp_file
1723939200.00,1,TX,screen_delta,0x03,0x02,200,120,40.00,heatshrink_w8_l4,150,raw/1.bin,comp/1.bin
1723939201.00,2,RX,client_input,0x02,0x00,10,10,0.00,none,0,raw/2.bin,comp/2.bin
"#;
        std::fs::write(temp_dir.join("compression_log.csv"), csv_content).unwrap();

        let mgr = StatsManager::new(&temp_dir);
        let (rows, total) = mgr.get_captured_packets(Some(10), Some(0));
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);
        let summary = mgr.get_capture_summary();
        assert_eq!(summary.total_samples, 2);
        assert_eq!(summary.total_raw_bytes, 210);
        assert_eq!(summary.total_comp_bytes, 130);
        assert_eq!(summary.avg_bytes_per_packet, 65.0);
        assert_eq!(summary.unique_users_count, 1);
        assert_eq!(summary.avg_bytes_per_packet_per_user, 65.0);
        assert!(summary.net_savings_percent > 35.0);
        assert_eq!(summary.tx_count, 1);
        assert_eq!(summary.rx_count, 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
