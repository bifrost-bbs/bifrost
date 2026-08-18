//! Log aggregation, filtering, ring buffer storage, and WebSocket streaming.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: String,
    pub source: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogQuery {
    pub level: Option<String>,
    pub source: Option<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub since_id: Option<u64>,
}

#[derive(Debug)]
pub struct LogBuffer {
    capacity: usize,
    entries: Mutex<VecDeque<LogEntry>>,
    next_id: std::sync::atomic::AtomicU64,
    tx: broadcast::Sender<LogEntry>,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(500);
        Self {
            capacity,
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
            next_id: std::sync::atomic::AtomicU64::new(1),
            tx,
        }
    }

    pub fn push(&self, source: &str, level: &str, message: &str) -> LogEntry {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let timestamp = chrono_now_iso();
        let entry = LogEntry {
            id,
            timestamp,
            source: source.to_string(),
            level: level.to_uppercase(),
            message: message.to_string(),
        };

        {
            let mut lock = self.entries.lock().unwrap();
            if lock.len() >= self.capacity {
                lock.pop_front();
            }
            lock.push_back(entry.clone());
        }

        // Broadcast to live streaming subscribers
        let _ = self.tx.send(entry.clone());

        entry
    }

    pub fn push_raw_line(&self, default_source: &str, raw_line: &str) -> LogEntry {
        let trimmed = raw_line.trim_end();
        let (level, source, msg) = parse_log_line(default_source, trimmed);
        self.push(&source, &level, &msg)
    }

    pub fn query(&self, query: &LogQuery) -> Vec<LogEntry> {
        let lock = self.entries.lock().unwrap();
        let limit = query.limit.unwrap_or(5000);

        let iter = lock.iter().rev().filter(|e| {
            if let Some(since) = query.since_id {
                if e.id <= since {
                    return false;
                }
            }
            if let Some(ref lvl) = query.level {
                if !lvl.is_empty() && !lvl.eq_ignore_ascii_case("ALL") && !e.level.eq_ignore_ascii_case(lvl) {
                    return false;
                }
            }
            if let Some(ref src) = query.source {
                if !src.is_empty() && !src.eq_ignore_ascii_case("ALL") && !e.source.eq_ignore_ascii_case(src) {
                    return false;
                }
            }
            if let Some(ref search) = query.search {
                if !search.is_empty() && !e.message.to_lowercase().contains(&search.to_lowercase()) {
                    return false;
                }
            }
            true
        });

        let mut results: Vec<LogEntry> = iter.take(limit).cloned().collect();
        results.reverse();
        results
    }

    pub fn clear(&self) {
        let mut lock = self.entries.lock().unwrap();
        lock.clear();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(5000)
    }
}

fn chrono_now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    
    // Format UTC ISO-8601 YYYY-MM-DDTHH:MM:SS.mmmZ
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let mins = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    // Approximate date for formatting
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hours, mins, seconds, millis
    )
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Gregorian cycle calculation
    let d = days + 719468;
    let era = d / 146097;
    let doe = d - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

fn parse_log_line(default_source: &str, line: &str) -> (String, String, String) {
    // Look for standard env_logger pattern: [TIMESTAMP LEVEL SOURCE] MSG or [LEVEL SOURCE] MSG
    if line.starts_with('[') {
        if let Some(close_bracket) = line.find(']') {
            let tag = &line[1..close_bracket];
            let rest = line[close_bracket + 1..].trim_start();
            let parts: Vec<&str> = tag.split_whitespace().collect();
            if parts.len() >= 2 {
                let level = parts[parts.len() - 2].to_uppercase();
                let source = parts[parts.len() - 1].to_string();
                if ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"].contains(&level.as_str()) {
                    return (level, source, rest.to_string());
                }
            } else if parts.len() == 1 {
                let level = parts[0].to_uppercase();
                if ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"].contains(&level.as_str()) {
                    return (level, default_source.to_string(), rest.to_string());
                }
            }
        }
    }

    // Infer level from text content if present
    let level = if line.contains("ERROR") || line.contains("error:") || line.contains("panic") {
        "ERROR"
    } else if line.contains("WARN") || line.contains("warning:") {
        "WARN"
    } else if line.contains("DEBUG") {
        "DEBUG"
    } else if line.contains("TRACE") {
        "TRACE"
    } else {
        "INFO"
    };

    (level.to_string(), default_source.to_string(), line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_buffer_push_and_query() {
        let buffer = LogBuffer::new(5);
        buffer.push("bifrost_bbs", "INFO", "Server started");
        buffer.push("bifrost_bbs", "WARN", "Rate limit warning");
        buffer.push("bifrost_client", "ERROR", "Connection refused");

        assert_eq!(buffer.len(), 3);

        let all = buffer.query(&LogQuery::default());
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].message, "Server started");
        assert_eq!(all[2].message, "Connection refused");

        // Filter by level
        let warn_only = buffer.query(&LogQuery {
            level: Some("WARN".to_string()),
            ..Default::default()
        });
        assert_eq!(warn_only.len(), 1);
        assert_eq!(warn_only[0].level, "WARN");

        // Filter by source
        let client_only = buffer.query(&LogQuery {
            source: Some("bifrost_client".to_string()),
            ..Default::default()
        });
        assert_eq!(client_only.len(), 1);
        assert_eq!(client_only[0].source, "bifrost_client");

        // Search text
        let search_res = buffer.query(&LogQuery {
            search: Some("limit".to_string()),
            ..Default::default()
        });
        assert_eq!(search_res.len(), 1);
        assert_eq!(search_res[0].message, "Rate limit warning");
    }

    #[test]
    fn test_log_buffer_ring_overflow() {
        let buffer = LogBuffer::new(3);
        buffer.push("src", "INFO", "Line 1");
        buffer.push("src", "INFO", "Line 2");
        buffer.push("src", "INFO", "Line 3");
        buffer.push("src", "INFO", "Line 4");

        assert_eq!(buffer.len(), 3);
        let items = buffer.query(&LogQuery::default());
        assert_eq!(items[0].message, "Line 2");
        assert_eq!(items[2].message, "Line 4");
    }

    #[test]
    fn test_parse_log_line() {
        let (lvl, src, msg) = parse_log_line("default", "[2026-08-18T12:00:00Z INFO bifrost_bbs] Server started on port 8088");
        assert_eq!(lvl, "INFO");
        assert_eq!(src, "bifrost_bbs");
        assert_eq!(msg, "Server started on port 8088");

        let (lvl2, src2, msg2) = parse_log_line("client", "Plain line without brackets");
        assert_eq!(lvl2, "INFO");
        assert_eq!(src2, "client");
        assert_eq!(msg2, "Plain line without brackets");
    }
}
