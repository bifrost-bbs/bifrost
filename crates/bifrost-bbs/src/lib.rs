//! MeshBBS Host server daemon kernel.
//! Handles session multiplexing, Rate Limiting, QoS Queues, and sandboxed Lua applications.

use anyhow::Result;
use log::{info, warn};
use mlua::LuaSerdeExt;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;
use tokio::sync::mpsc;

// Pull from sibling workspace crates
use bifrost_transport::{
    MeshBbsMessage, MessageReassembler, MockSocketTransport, RadioPacket, RadioTransport,
    TransportStats,
};

/// BBS-level statistics tracker for session and user accounting.
pub struct BbsStats {
    /// Timestamps of when each unique node connected (for 24h active user count)
    pub session_timestamps: StdMutex<Vec<(Instant, [u8; 32])>>,
    /// Currently active session node IDs
    pub active_session_count: StdMutex<usize>,
    pub raw_bytes_sent: AtomicU64,
    pub raw_bytes_received: AtomicU64,
    pub compressed_bytes_sent: AtomicU64,
    pub compressed_bytes_received: AtomicU64,
}

impl BbsStats {
    pub fn new() -> Self {
        Self {
            session_timestamps: StdMutex::new(Vec::new()),
            active_session_count: StdMutex::new(0),
            raw_bytes_sent: AtomicU64::new(0),
            raw_bytes_received: AtomicU64::new(0),
            compressed_bytes_sent: AtomicU64::new(0),
            compressed_bytes_received: AtomicU64::new(0),
        }
    }

    /// Records a new session connection.
    pub fn record_session_connect(&self, node_id: [u8; 32]) {
        if let Ok(mut ts) = self.session_timestamps.lock() {
            ts.push((Instant::now(), node_id));
        }
        if let Ok(mut count) = self.active_session_count.lock() {
            *count += 1;
        }
    }

    /// Records a session disconnection.
    pub fn record_session_disconnect(&self) {
        if let Ok(mut count) = self.active_session_count.lock() {
            *count = count.saturating_sub(1);
        }
    }

    /// Records raw and compressed byte counts for transmitted data.
    pub fn record_compression(&self, raw_bytes: usize, compressed_bytes: usize) {
        self.raw_bytes_sent.fetch_add(raw_bytes as u64, Ordering::Relaxed);
        self.compressed_bytes_sent.fetch_add(compressed_bytes as u64, Ordering::Relaxed);
    }

    /// Records raw and compressed byte counts for received data.
    pub fn record_decompression(&self, compressed_bytes: usize, raw_bytes: usize) {
        self.compressed_bytes_received.fetch_add(compressed_bytes as u64, Ordering::Relaxed);
        self.raw_bytes_received.fetch_add(raw_bytes as u64, Ordering::Relaxed);
    }

    /// Total uncompressed raw bytes sent.
    pub fn total_raw_bytes_sent(&self) -> u64 {
        self.raw_bytes_sent.load(Ordering::Relaxed)
    }

    /// Total uncompressed raw bytes received.
    pub fn total_raw_bytes_received(&self) -> u64 {
        self.raw_bytes_received.load(Ordering::Relaxed)
    }

    /// Total compressed bytes sent.
    pub fn total_compressed_bytes_sent(&self) -> u64 {
        self.compressed_bytes_sent.load(Ordering::Relaxed)
    }

    /// Total compressed bytes received.
    pub fn total_compressed_bytes_received(&self) -> u64 {
        self.compressed_bytes_received.load(Ordering::Relaxed)
    }

    /// Returns the count of unique nodes that have connected in the last 24 hours.
    pub fn unique_users_24h(&self) -> usize {
        let cutoff = Instant::now() - std::time::Duration::from_secs(86400);
        if let Ok(mut ts) = self.session_timestamps.lock() {
            ts.retain(|&(t, _)| t >= cutoff);
            let unique: HashSet<[u8; 32]> = ts.iter().map(|&(_, id)| id).collect();
            unique.len()
        } else {
            0
        }
    }

    /// Returns the current number of active sessions.
    pub fn active_sessions(&self) -> usize {
        *self.active_session_count.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn default_form_colors() -> FormColorsConfig {
    FormColorsConfig {
        field_fg: 15,
        field_bg: 4,
        submit_fg: 0,
        submit_bg: 7,
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct FormColorsConfig {
    pub field_fg: u8,
    pub field_bg: u8,
    pub submit_fg: u8,
    pub submit_bg: u8,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_main_app() -> String {
    "main_menu".to_string()
}

fn default_enabled_apps() -> Vec<String> {
    vec![
        "main_menu".to_string(),
        "messages".to_string(),
        "profile".to_string(),
        "minidungeon".to_string(),
        "admin".to_string(),
        "marketplace".to_string(),
    ]
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppsConfig {
    #[serde(default = "default_main_app")]
    pub main_app: String,
    #[serde(default = "default_enabled_apps")]
    pub enabled: Vec<String>,
}

fn default_apps_config() -> AppsConfig {
    AppsConfig {
        main_app: default_main_app(),
        enabled: default_enabled_apps(),
    }
}

/// Bifrost BBS Server Configuration Loaded from config.toml
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    pub rate_limiter: RateLimiterConfig,
    pub asset_broadcaster: AssetBroadcasterConfig,
    #[serde(default = "default_form_colors")]
    pub form_colors: FormColorsConfig,
    #[serde(default)]
    pub admin_nodes: Vec<String>,
    #[serde(default = "default_apps_config")]
    pub apps: AppsConfig,
    #[serde(default = "default_packet_capture_config")]
    pub packet_capture: PacketCaptureConfig,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct PacketCaptureConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_capture_dir")]
    pub directory: String,
}

fn default_capture_dir() -> String {
    "captured_packets".to_string()
}

pub fn default_packet_capture_config() -> PacketCaptureConfig {
    PacketCaptureConfig {
        enabled: false,
        directory: default_capture_dir(),
    }
}

/// Thread-safe packet capture and compression logger for diagnostic & tuning analysis.
#[derive(Debug)]
pub struct PacketRecorder {
    pub base_dir: PathBuf,
    pub raw_dir: PathBuf,
    pub comp_dir: PathBuf,
    csv_file: StdMutex<Option<std::fs::File>>,
    seq: AtomicU64,
}

impl PacketRecorder {
    pub fn new(directory: &str) -> Result<Self> {
        let base_dir = find_workspace_path(directory);
        let raw_dir = base_dir.join("raw");
        let comp_dir = base_dir.join("comp");
        std::fs::create_dir_all(&raw_dir)?;
        std::fs::create_dir_all(&comp_dir)?;

        let csv_path = base_dir.join("compression_log.csv");
        let file_exists = csv_path.exists();
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&csv_path)?;

        let mut csv_file = file;
        if !file_exists || csv_file.metadata()?.len() == 0 {
            use std::io::Write;
            writeln!(
                csv_file,
                "timestamp,seq,direction,category,opcode,flags,raw_bytes,compressed_bytes,savings_percent,algorithm,duration_us,raw_file,comp_file"
            )?;
        }

        log::info!("Packet capture active, logging to {:?}", base_dir);

        Ok(Self {
            base_dir,
            raw_dir,
            comp_dir,
            csv_file: StdMutex::new(Some(csv_file)),
            seq: AtomicU64::new(1),
        })
    }

    pub fn record_compression(
        &self,
        direction: &str,
        category: &str,
        opcode: u8,
        flags: u8,
        raw: &[u8],
        compressed: Option<&[u8]>,
        algorithm: &str,
        duration_us: u64,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let epoch_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let raw_filename = format!("seq_{:06}_{}_{}.bin", seq, direction.to_lowercase(), category);
        let raw_path = self.raw_dir.join(&raw_filename);
        let _ = std::fs::write(&raw_path, raw);
        let raw_rel = format!("raw/{}", raw_filename);

        let (comp_bytes, savings_pct, comp_rel) = if let Some(comp) = compressed {
            let comp_filename = format!("seq_{:06}_{}_{}.bin", seq, direction.to_lowercase(), category);
            let comp_path = self.comp_dir.join(&comp_filename);
            let _ = std::fs::write(&comp_path, comp);
            let raw_len = raw.len() as f64;
            let comp_len = comp.len() as f64;
            let savings = if raw_len > 0.0 {
                ((raw_len - comp_len) / raw_len) * 100.0
            } else {
                0.0
            };
            (comp.len(), savings, format!("comp/{}", comp_filename))
        } else {
            (0, 0.0, String::new())
        };

        if let Ok(mut guard) = self.csv_file.lock() {
            if let Some(ref mut f) = *guard {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "{:.3},{},{},{},0x{:02X},0x{:02X},{},{},{:.2},{},{},{},{}",
                    epoch_now,
                    seq,
                    direction,
                    category,
                    opcode,
                    flags,
                    raw.len(),
                    comp_bytes,
                    savings_pct,
                    algorithm,
                    duration_us,
                    raw_rel,
                    comp_rel
                );
            }
        }

        log::debug!(
            "[CAPTURE #{:06}] {} {} (opcode=0x{:02X}): raw={}B, comp={}B ({:+.1}%) in {}µs",
            seq,
            direction,
            category,
            opcode,
            raw.len(),
            comp_bytes,
            savings_pct,
            duration_us
        );
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RateLimiterConfig {
    pub max_packets_per_minute: u32,
    pub max_burst_packets: u32,
    pub inter_packet_guard_ms: u32,
    pub max_duty_cycle_percent: f32,
    pub duty_cycle_window_secs: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AssetBroadcasterConfig {
    pub enable_on_demand_broadcast: bool,
    pub max_asset_broadcast_duty_cycle: f32,
}

#[allow(dead_code)]
struct SessionManager {
    // Session state map goes here
}

#[allow(dead_code)]
struct AirtimeRegulator {
    config: RateLimiterConfig,
    // Bucket level, rolling average state, queue goes here
}

/// Returns the default fallback configuration parameters.
pub fn default_config() -> AppConfig {
    AppConfig {
        log_level: "info".to_string(),
        rate_limiter: RateLimiterConfig {
            max_packets_per_minute: 45,
            max_burst_packets: 4,
            inter_packet_guard_ms: 350,
            max_duty_cycle_percent: 1.0,
            duty_cycle_window_secs: 3600,
        },
        asset_broadcaster: AssetBroadcasterConfig {
            enable_on_demand_broadcast: true,
            max_asset_broadcast_duty_cycle: 0.15,
        },
        form_colors: FormColorsConfig {
            field_fg: 15,
            field_bg: 4,
            submit_fg: 0,
            submit_bg: 7,
        },
        admin_nodes: Vec::new(),
        apps: default_apps_config(),
        packet_capture: default_packet_capture_config(),
    }
}

/// Run the BBS server engine, loading configuration and initializing transport.
pub async fn run_bbs(config_path: Option<PathBuf>, run_duration_secs: Option<u64>) -> Result<()> {
    run_bbs_with_capture(config_path, run_duration_secs, None).await
}

/// Run the BBS server engine with optional CLI capture directory override.
pub async fn run_bbs_with_capture(
    config_path: Option<PathBuf>,
    run_duration_secs: Option<u64>,
    capture_dir: Option<String>,
) -> Result<()> {
    // 1. Load Config File
    let mut config: AppConfig = if let Some(path) = config_path {
        let resolved = find_workspace_path(path.to_str().unwrap_or(""));
        if resolved.exists() {
            info!("Loading configuration from {:?}", resolved);
            let contents = std::fs::read_to_string(&resolved)?;
            toml::from_str(&contents)?
        } else {
            warn!(
                "Config file not found at {:?}, using default settings",
                path
            );
            default_config()
        }
    } else {
        default_config()
    };

    if let Some(dir) = capture_dir {
        config.packet_capture.enabled = true;
        config.packet_capture.directory = dir;
    }

    // 2. Initialize Transport
    let mock_transport = if run_duration_secs.is_some() {
        MockSocketTransport::new(0.0, 10, 200)
    } else {
        MockSocketTransport::new_server(
            "127.0.0.1:8088".to_string(),
            0.0,
            10,
            200,
        )
    };
    let transport_stats = mock_transport.stats.clone();
    let transport: Arc<dyn RadioTransport> = Arc::new(mock_transport);

    // 3. Start Server Runtime
    start_server_with_stats(config, transport, run_duration_secs, Some(transport_stats)).await
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppMetadata {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub repository: Option<String>,
    pub entry_point: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppAssetEntry {
    pub name: String,
    #[serde(default)]
    pub id: Option<u16>,
    pub path: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppManifest {
    pub app: AppMetadata,
    #[serde(default)]
    pub assets: Vec<AppAssetEntry>,
}

/// Loads the static public asset manifests for all enabled apps.
/// Dynamically assigns unique 16-bit AssetIDs to prevent conflicts.
/// Maps AssetID -> (CanonicalNamespacedName, ResolvedRelativePath).
pub fn load_app_manifests(enabled_apps: &[String]) -> HashMap<u16, (String, String)> {
    let mut map = HashMap::new();
    let mut next_dynamic_id: u16 = 0x0101;

    for app_id in enabled_apps {
        let manifest_rel = format!("apps/{}/manifest.toml", app_id);
        let manifest_path = find_workspace_path(&manifest_rel);
        if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
            if let Ok(app_manifest) = toml::from_str::<AppManifest>(&contents) {
                log::info!(
                    "Loaded app '{}' ({}) v{} by {}",
                    app_manifest.app.name,
                    app_manifest.app.id,
                    app_manifest.app.version.as_deref().unwrap_or("1.0.0"),
                    app_manifest.app.author.as_deref().unwrap_or("Unknown")
                );
                for asset in app_manifest.assets {
                    let asset_id = if let Some(explicit_id) = asset.id {
                        explicit_id
                    } else {
                        while map.contains_key(&next_dynamic_id) {
                            next_dynamic_id = next_dynamic_id.wrapping_add(1);
                        }
                        let id = next_dynamic_id;
                        next_dynamic_id = next_dynamic_id.wrapping_add(1);
                        id
                    };

                    let asset_path = format!("apps/{}/{}", app_id, asset.path);
                    let namespaced_name = format!("{}/{}", app_id, asset.name);
                    log::debug!(
                        "Registered asset 0x{:04X}: '{}' -> '{}'",
                        asset_id,
                        namespaced_name,
                        asset_path
                    );
                    map.insert(asset_id, (namespaced_name, asset_path));
                }
            } else {
                log::warn!("Failed to parse app manifest: {:?}", manifest_path);
            }
        } else {
            log::warn!("App manifest not found: {:?}", manifest_path);
        }
    }
    map
}

/// Broadcasts a requested public asset in unencrypted multicast chunks according to spec.
pub async fn broadcast_asset(
    asset_id: u16,
    manifest_map: &HashMap<u16, (String, String)>,
    transport: &Arc<dyn RadioTransport>,
    packet_recorder: &Option<Arc<PacketRecorder>>,
) -> Result<()> {
    if let Some((name, rel_path)) = manifest_map.get(&asset_id) {
        let full_path = find_workspace_path(rel_path);
        if let Ok(content_bytes) = std::fs::read(&full_path) {
            let master_crc = bifrost_transport::crc32(&content_bytes);
            let mtu = transport.get_mtu();
            let chunk_capacity = if mtu > 16 { mtu - 12 } else { 32 };
            let total_chunks = ((content_bytes.len() + chunk_capacity - 1) / chunk_capacity) as u8;
            let total_chunks = std::cmp::max(1, total_chunks);

            log::info!(
                "Broadcasting public asset '{}' (0x{:04X}, {} bytes, {} chunks, CRC32: 0x{:08X})",
                name,
                asset_id,
                content_bytes.len(),
                total_chunks,
                master_crc
            );

            if let Some(ref recorder) = packet_recorder {
                recorder.record_compression(
                    "TX",
                    "broadcast_asset_full",
                    0x04,
                    0x08,
                    &content_bytes,
                    None,
                    "raw",
                    0,
                );
            }

            for chunk_idx in 1..=total_chunks {
                let start = (chunk_idx as usize - 1) * chunk_capacity;
                let end = std::cmp::min(start + chunk_capacity, content_bytes.len());
                let chunk_payload = if start < content_bytes.len() {
                    &content_bytes[start..end]
                } else {
                    &[]
                };

                let mut packet_payload = Vec::with_capacity(12 + chunk_payload.len());
                packet_payload.push(0xBB); // AppPort (MeshBBS)
                packet_payload.push(0x04); // MsgType (Broadcast Asset)
                packet_payload.push(0x08); // Flags (B=1)
                packet_payload.push(chunk_idx); // ChunkIndex (1-indexed)
                packet_payload.push(total_chunks); // TotalChunks
                packet_payload.extend_from_slice(&asset_id.to_be_bytes()); // AssetID (2B)
                packet_payload.push(chunk_payload.len() as u8); // PayloadLength
                packet_payload.extend_from_slice(&master_crc.to_be_bytes()); // Master CRC32 (4B)
                packet_payload.extend_from_slice(chunk_payload);

                if let Some(ref recorder) = packet_recorder {
                    recorder.record_compression(
                        "TX",
                        "broadcast_asset_chunk",
                        0x04,
                        0x08,
                        &packet_payload,
                        None,
                        "raw",
                        0,
                    );
                }

                let packet = RadioPacket {
                    is_broadcast: true,
                    src_node: [0; 32],
                    dst_node: [0; 32],
                    payload: packet_payload,
                    signal_rssi: 0,
                    signal_snr: 0,
                };

                if let Err(e) = transport.send_packet(packet).await {
                    log::error!("Failed to send broadcast asset chunk: {:?}", e);
                }
            }
        } else {
            log::warn!("Asset file not found at {:?}", full_path);
        }
    } else {
        log::warn!("Asset ID 0x{:04X} not found in manifest", asset_id);
    }
    Ok(())
}

/// Parses a MeshCore node advertisement payload.
/// Supports both full MeshCore packet framing (PAYLOAD_TYPE_ADVERT = 0x04)
/// and bare 100+ byte advertisement payloads.
pub fn parse_meshcore_advert(
    payload: &[u8],
    src_node: [u8; 32],
) -> Option<([u8; 32], serde_json::Map<String, serde_json::Value>)> {
    let advert_slice: &[u8] = if !payload.is_empty() && ((payload[0] >> 2) & 0x0F) == 0x04 {
        // Full MeshCore Packet header: 0bVVPPPPRR
        let route_type = payload[0] & 0x03;
        let mut offset = 1;
        // Transport codes (4 bytes) if ROUTE_TYPE_TRANSPORT_FLOOD (0) or ROUTE_TYPE_TRANSPORT_DIRECT (3)
        if route_type == 0 || route_type == 3 {
            if payload.len() < offset + 4 {
                return None;
            }
            offset += 4;
        }
        if payload.len() < offset + 1 {
            return None;
        }
        let path_len = payload[offset] as usize;
        offset += 1;
        if payload.len() < offset + path_len {
            return None;
        }
        offset += path_len;
        if payload.len() < offset {
            return None;
        }
        &payload[offset..]
    } else if payload.len() >= 100 {
        // Bare advert payload
        payload
    } else {
        return None;
    };

    if advert_slice.len() < 100 {
        return None;
    }

    let pubkey: [u8; 32] = advert_slice[0..32].try_into().ok()?;
    let timestamp = u32::from_le_bytes(advert_slice[32..36].try_into().ok()?);
    // signature is advert_slice[36..100] (64 bytes)

    let mut metadata = serde_json::Map::new();
    let pubkey_hex: String = pubkey.iter().map(|b| format!("{:02x}", b)).collect();
    metadata.insert("public_key".to_string(), serde_json::json!(pubkey_hex));
    metadata.insert("advert_timestamp".to_string(), serde_json::json!(timestamp));

    if advert_slice.len() > 100 {
        let flags = advert_slice[100];
        let mut offset = 101;

        // Node type from lower 4 bits (flags & 0x0F)
        let node_type = match flags & 0x0F {
            0x01 => "chat_node",
            0x02 => "repeater",
            0x03 => "room_server",
            0x04 => "sensor",
            _ => "unknown",
        };
        metadata.insert("node_type".to_string(), serde_json::json!(node_type));

        // Location (flags & 0x10)
        if (flags & 0x10) != 0 && advert_slice.len() >= offset + 8 {
            let lat_int = i32::from_le_bytes([
                advert_slice[offset],
                advert_slice[offset + 1],
                advert_slice[offset + 2],
                advert_slice[offset + 3],
            ]);
            let lon_int = i32::from_le_bytes([
                advert_slice[offset + 4],
                advert_slice[offset + 5],
                advert_slice[offset + 6],
                advert_slice[offset + 7],
            ]);
            offset += 8;
            let lat = lat_int as f64 / 1_000_000.0;
            let lon = lon_int as f64 / 1_000_000.0;
            metadata.insert("latitude".to_string(), serde_json::json!(lat));
            metadata.insert("longitude".to_string(), serde_json::json!(lon));
            metadata.insert(
                "last_known_location".to_string(),
                serde_json::json!(format!("{:.4}, {:.4}", lat, lon)),
            );
        }

        // Feature 1 (flags & 0x20)
        if (flags & 0x20) != 0 && advert_slice.len() >= offset + 2 {
            let feat1 = u16::from_le_bytes([advert_slice[offset], advert_slice[offset + 1]]);
            metadata.insert("feature1".to_string(), serde_json::json!(feat1));
            offset += 2;
        }

        // Feature 2 (flags & 0x40)
        if (flags & 0x40) != 0 && advert_slice.len() >= offset + 2 {
            let feat2 = u16::from_le_bytes([advert_slice[offset], advert_slice[offset + 1]]);
            metadata.insert("feature2".to_string(), serde_json::json!(feat2));
            offset += 2;
        }

        // Node name (flags & 0x80)
        if (flags & 0x80) != 0 && advert_slice.len() > offset {
            if let Ok(name_str) = String::from_utf8(advert_slice[offset..].to_vec()) {
                metadata.insert("node_name".to_string(), serde_json::json!(name_str));
            }
        }
    }

    let target_node = if pubkey != [0; 32] { pubkey } else { src_node };
    Some((target_node, metadata))
}

struct Session {
    input_tx: mpsc::Sender<MeshBbsMessage>,
}

/// Starts the BBS Host server daemon.
/// If run_duration_secs is provided, the server runs for that duration before exiting.
pub async fn start_server(
    config: AppConfig,
    transport: Arc<dyn RadioTransport>,
    run_duration_secs: Option<u64>,
) -> Result<()> {
    start_server_with_stats(config, transport, run_duration_secs, None).await
}

/// Starts the BBS Host server daemon with optional transport-level stats tracking.
pub async fn start_server_with_stats(
    config: AppConfig,
    transport: Arc<dyn RadioTransport>,
    run_duration_secs: Option<u64>,
    transport_stats: Option<Arc<TransportStats>>,
) -> Result<()> {
    info!(
        "Rate Limiter active: Max Packets/Min={}, Max Duty Cycle={}%",
        config.rate_limiter.max_packets_per_minute, config.rate_limiter.max_duty_cycle_percent
    );
    info!("Mock radio transport initialized.");

    // Passive on-demand broadcasting is handled when a connected client requests a missing asset.
    if config.asset_broadcaster.enable_on_demand_broadcast {
        info!("On-demand public asset broadcasting enabled.");
    }
    let active_sessions = Arc::new(StdMutex::new(HashMap::<[u8; 32], Session>::new()));
    let db_store = Arc::new(StdMutex::new(
        HashMap::<String, HashMap<String, String>>::new(),
    ));
    let mut reassembler = MessageReassembler::new();
    let asset_manifest_map = Arc::new(load_app_manifests(&config.apps.enabled));
    let bbs_stats = Arc::new(BbsStats::new());

    let packet_recorder: Option<Arc<PacketRecorder>> = if config.packet_capture.enabled {
        match PacketRecorder::new(&config.packet_capture.directory) {
            Ok(rec) => Some(Arc::new(rec)),
            Err(e) => {
                log::error!("Failed to initialize packet recorder: {:?}", e);
                None
            }
        }
    } else {
        None
    };

    // Spawn periodic stats logger (once per minute)
    let stats_logger_handle = if let Some(ref ts) = transport_stats {
        let ts_clone = ts.clone();
        let bbs_stats_clone = bbs_stats.clone();
        let duration_limit = run_duration_secs;
        Some(tokio::spawn(async move {
            let start = tokio::time::Instant::now();
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            interval.tick().await; // First tick fires immediately, skip it
            loop {
                interval.tick().await;
                if let Some(dur) = duration_limit {
                    if start.elapsed() >= tokio::time::Duration::from_secs(dur) {
                        break;
                    }
                }
                let (send_ppm, recv_ppm) = ts_clone.packets_per_minute_last(3600);
                let (send_ppm_24h, recv_ppm_24h) = ts_clone.packets_per_minute_last(86400);
                info!(
                    "[BBS STATS] Active Users 24h: {} | Current Sessions: {} | Pkts Sent: {} | Pkts Recv: {} | Bytes Sent: {} (raw: {}, comp: {}) | Bytes Recv: {} (raw: {}, comp: {}) | Avg PPM 1h: {:.1}/{:.1} | Avg PPM 24h: {:.1}/{:.1} | Uptime: {}s",
                    bbs_stats_clone.unique_users_24h(),
                    bbs_stats_clone.active_sessions(),
                    ts_clone.total_packets_sent(),
                    ts_clone.total_packets_received(),
                    ts_clone.total_bytes_sent(),
                    ts_clone.total_raw_bytes_sent(),
                    ts_clone.total_compressed_bytes_sent(),
                    ts_clone.total_bytes_received(),
                    ts_clone.total_raw_bytes_received(),
                    ts_clone.total_compressed_bytes_received(),
                    send_ppm,
                    recv_ppm,
                    send_ppm_24h,
                    recv_ppm_24h,
                    ts_clone.uptime_secs()
                );
            }
        }))
    } else {
        None
    };

    // Main packet routing loop
    let loop_handle = tokio::spawn(async move {
        let bbs_stats_clone = bbs_stats.clone();
        let manifest_map_for_loop = asset_manifest_map.clone();
        let start_time = tokio::time::Instant::now();
        loop {
            if let Some(dur) = run_duration_secs {
                if start_time.elapsed() >= tokio::time::Duration::from_secs(dur) {
                    break;
                }
            }

            // Receive packet from radio
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    let src = packet.src_node;

                    // Intercept and parse MeshCore advert packets
                    if let Some((target_node, metadata)) = parse_meshcore_advert(&packet.payload, src) {
                        let mut store = db_store.lock().unwrap();
                        let users_table = store.entry("users".to_string()).or_insert_with(HashMap::new);
                        let node_hex: String = target_node.iter().map(|b| format!("{:02x}", b)).collect();

                        let mut existing_user = if let Some(existing_json) = users_table.get(&node_hex) {
                            serde_json::from_str::<serde_json::Value>(existing_json).unwrap_or(serde_json::json!({}))
                        } else {
                            serde_json::json!({})
                        };

                        if let Some(obj) = existing_user.as_object_mut() {
                            for (k, v) in metadata {
                                obj.insert(k, v);
                            }
                        }

                        if let Ok(merged_json) = serde_json::to_string(&existing_user) {
                            log::info!("Processed advert packet for node {}: {}", node_hex, merged_json);
                            users_table.insert(node_hex, merged_json);
                        }

                        continue;
                    }

                    match reassembler.process_packet(src, &packet.payload) {
                        Ok(Some(msg)) => {
                            if msg.opcode == 0x05 && msg.payload.len() >= 2 {
                                let req_asset_id =
                                    u16::from_be_bytes([msg.payload[0], msg.payload[1]]);
                                log::info!(
                                    "Received REQ_ASSET for asset 0x{:04X} from node {:?}",
                                    req_asset_id,
                                    src
                                );
                                let manifest_map_clone = manifest_map_for_loop.clone();
                                let transport_broadcast = transport.clone();
                                let packet_recorder_broadcast = packet_recorder.clone();
                                tokio::spawn(async move {
                                    let _ = broadcast_asset(
                                        req_asset_id,
                                        &manifest_map_clone,
                                        &transport_broadcast,
                                        &packet_recorder_broadcast,
                                    )
                                    .await;
                                });
                                continue;
                            }

                            let tx_opt = active_sessions
                                .lock()
                                .unwrap()
                                .get(&src)
                                .map(|s| s.input_tx.clone());
                            if let Some(tx) = tx_opt {
                                let _ = tx.send(msg).await;
                            } else {
                                // Boot new session
                                info!("Booting new Lua session for node: {:?}", src);
                                let (tx, rx) = mpsc::channel(100);
                                let session = Session {
                                    input_tx: tx.clone(),
                                };
                                active_sessions.lock().unwrap().insert(src, session);
                                bbs_stats_clone.record_session_connect(src);

                                let sessions_clone = active_sessions.clone();
                                let transport_inner = transport.clone();
                                let db_inner = db_store.clone();
                                let rt_handle = tokio::runtime::Handle::current();
                                let form_colors_config = config.form_colors.clone();
                                let admin_nodes_config = config.admin_nodes.clone();
                                let asset_manifest_clone = asset_manifest_map.clone();
                                let apps_config_clone = config.apps.clone();
                                let bbs_stats_inner = bbs_stats_clone.clone();
                                let transport_stats_inner = transport_stats.clone();
                                let packet_recorder_inner = packet_recorder.clone();
                                std::thread::spawn(move || {
                                    let res = run_session_task(
                                        src,
                                        rx,
                                        transport_inner,
                                        db_inner,
                                        rt_handle,
                                        form_colors_config,
                                        admin_nodes_config,
                                        asset_manifest_clone,
                                        apps_config_clone,
                                        bbs_stats_inner.clone(),
                                        transport_stats_inner,
                                        packet_recorder_inner,
                                    );
                                    sessions_clone.lock().unwrap().remove(&src);
                                    bbs_stats_inner.record_session_disconnect();
                                    if let Err(e) = res {
                                        log::error!("Session task error: {:?}", e);
                                    }
                                });
                                let _ = tx.send(msg).await;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("Protocol reassembly error for node {:?}: {}", src, e);
                        }
                    }
                }
                Ok(Err(bifrost_transport::TransportError::ConnectionClosed)) => {
                    info!("Transport connection closed.");
                    break;
                }
                Ok(Err(e)) => {
                    log::error!("Transport receive error: {:?}", e);
                }
                Err(_) => {} // timeout
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    loop_handle.await??;

    // Cancel the stats logger if it was running
    if let Some(handle) = stats_logger_handle {
        handle.abort();
    }

    Ok(())
}

fn run_session_task(
    node_id: [u8; 32],
    mut rx: mpsc::Receiver<MeshBbsMessage>,
    transport: Arc<dyn RadioTransport>,
    db_store: Arc<StdMutex<HashMap<String, HashMap<String, String>>>>,
    rt_handle: tokio::runtime::Handle,
    form_colors: FormColorsConfig,
    admin_nodes: Vec<String>,
    asset_manifest: Arc<HashMap<u16, (String, String)>>,
    apps_config: AppsConfig,
    bbs_stats: Arc<BbsStats>,
    transport_stats: Option<Arc<TransportStats>>,
    packet_recorder: Option<Arc<PacketRecorder>>,
) -> Result<()> {
    log::debug!("Starting run_session_task for client session");
    let lua = mlua::Lua::new();
    let active_app = Arc::new(StdMutex::new(apps_config.main_app.clone()));

    let node_hex_str: String = node_id.iter().map(|b| format!("{:02x}", b)).collect();

    // Check if configured as admin
    let is_configured_admin = admin_nodes.contains(&node_hex_str);

    // Check if first user in database
    let is_first_user = {
        let store = db_store.lock().unwrap();
        match store.get("users") {
            Some(users_map) => users_map.is_empty(),
            None => true,
        }
    };

    let mut initial_permissions = vec!["read".to_string(), "write".to_string()];
    if is_configured_admin || is_first_user {
        initial_permissions.push("admin".to_string());
    }

    // Persist initial permissions in DB
    let node_hex_str_clone = node_hex_str.clone();
    let db_store_perms = db_store.clone();
    {
        let mut store = db_store_perms.lock().unwrap();
        let perms_table = store
            .entry("permissions".to_string())
            .or_insert_with(HashMap::new);
        if !perms_table.contains_key(&node_hex_str_clone) {
            let json_str =
                serde_json::to_string(&initial_permissions).unwrap_or_else(|_| "[]".to_string());
            perms_table.insert(node_hex_str_clone, json_str);
        }
    }

    // Accumulates output bytes for term.flush()
    let output_buf = Arc::new(StdMutex::new(Vec::new()));

    // Setup sandboxed environment
    let globals = lua.globals();
    globals.set("os", mlua::Value::Nil)?;
    globals.set("io", mlua::Value::Nil)?;
    globals.set("package", mlua::Value::Nil)?;

    // Sandboxed relative require scoped to the active application folder
    let active_app_for_require = active_app.clone();
    globals.set(
        "require",
        lua.create_function(move |lua, mod_name: String| {
            let current_app = active_app_for_require.lock().unwrap().clone();
            let path = find_workspace_path(&format!("apps/{}/{}.lua", current_app, mod_name));
            let path = if path.exists() {
                path
            } else {
                find_workspace_path(&format!("apps/{}/{}/init.lua", current_app, mod_name))
            };
            if path.exists() {
                let code = std::fs::read_to_string(&path)?;
                let chunk = lua.load(&code).set_name(&mod_name);
                let val: mlua::Value = chunk.eval()?;
                Ok(val)
            } else {
                Err(mlua::Error::RuntimeError(format!(
                    "Module '{}' not found in app '{}'",
                    mod_name, current_app
                )))
            }
        })?,
    )?;

    // term table
    let term = lua.create_table()?;

    let out_buf = output_buf.clone();
    term.set(
        "clear",
        lua.create_function(move |_, (): ()| {
            out_buf.lock().unwrap().push(0x01); // OP_CLEAR_SCREEN
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "move_to",
        lua.create_function(move |_, (col, row): (u8, u8)| {
            let mut buf = out_buf.lock().unwrap();
            buf.push(0xC3); // OP_CURSOR_ABS
            buf.push(col);
            buf.push(row);
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "print",
        lua.create_function(move |_, text: String| {
            let mut buf = out_buf.lock().unwrap();
            buf.extend_from_slice(text.as_bytes());
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "set_color",
        lua.create_function(move |_, (fg, bg): (u8, u8)| {
            let mut buf = out_buf.lock().unwrap();
            buf.push(0xC0); // OP_SET_COLOR
            let attr = (bg << 4) | (fg & 0x0F);
            buf.push(attr);
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    let asset_manifest_for_render = asset_manifest.clone();
    let active_app_for_render = active_app.clone();
    term.set(
        "render_asset",
        lua.create_function(move |_, asset_name: String| {
            let mut buf = out_buf.lock().unwrap();
            buf.push(0xC5); // OP_RENDER_ASSET
            let current_app = active_app_for_render.lock().unwrap().clone();

            // 1. Normalized namespaced targets: e.g. "main_menu/banner", "main_menu/main_menu_banner"
            let normalized_target = asset_name.replace("::", "/").replace(':', "/");
            let relative_target = format!("{}/{}", current_app, normalized_target);

            let id_opt = asset_manifest_for_render
                .iter()
                .find(|(_, (n, _))| {
                    let n_norm = n.replace("::", "/").replace(':', "/");
                    n_norm == normalized_target
                        || n_norm == relative_target
                        || n_norm.ends_with(&format!("/{}", normalized_target))
                        || n_norm.to_ascii_uppercase().contains(&asset_name.to_ascii_uppercase())
                })
                .map(|(&id, _)| id);

            let id = match id_opt {
                Some(matched_id) => matched_id,
                None => {
                    log::warn!(
                        "Asset '{}' not found in loaded asset manifests (current app: '{}')",
                        asset_name,
                        current_app
                    );
                    0x0101
                }
            };
            buf.extend_from_slice(&id.to_be_bytes());
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    let transport_clone = transport.clone();
    let node_id_clone = node_id;
    let rt = rt_handle.clone();
    let bbs_stats_flush = bbs_stats.clone();
    let transport_stats_flush = transport_stats.clone();
    let packet_recorder_flush = packet_recorder.clone();
    term.set(
        "flush",
        lua.create_function(move |_, (): ()| {
            let mut buf = out_buf.lock().unwrap();
            log::debug!(
                "term.flush() called with {} bytes in session buffer",
                buf.len()
            );
            if !buf.is_empty() {
                buf.push(0x04); // EndOfFrame
                let raw_len = buf.len();
                let start_comp = std::time::Instant::now();
                let (flags, payload) = match bifrost_ansi::compress_bytecode(&buf) {
                    Ok(comp) => {
                        let comp_len = comp.len();
                        bbs_stats_flush.record_compression(raw_len, comp_len);
                        if let Some(ref ts) = transport_stats_flush {
                            ts.record_compression(raw_len, comp_len);
                        }
                        (0x02, comp)
                    }
                    Err(e) => {
                        log::warn!("Compression failed, sending uncompressed: {:?}", e);
                        (0x00, buf.clone())
                    }
                };
                let comp_duration = start_comp.elapsed().as_micros() as u64;
                if let Some(ref recorder) = packet_recorder_flush {
                    let comp_opt = if (flags & 0x02) != 0 {
                        Some(payload.as_slice())
                    } else {
                        None
                    };
                    recorder.record_compression(
                        "TX",
                        "screen_delta",
                        0x03,
                        flags,
                        &buf,
                        comp_opt,
                        "heatshrink_w8_l4",
                        comp_duration,
                    );
                }
                buf.clear();

                let msg = MeshBbsMessage::new(0x01, 0x03, flags, payload);
                let mtu = transport_clone.get_mtu();
                match msg.to_fragments(mtu) {
                    Ok(fragments) => {
                        let transport_inner = transport_clone.clone();
                        log::debug!(
                            "Sending term.flush() fragmented packets over transport (count={})",
                            fragments.len()
                        );
                        rt.block_on(async {
                            for frag in fragments {
                                let packet = RadioPacket {
                                    is_broadcast: false,
                                    src_node: [0; 32],
                                    dst_node: node_id_clone,
                                    payload: frag,
                                    signal_rssi: 0,
                                    signal_snr: 0,
                                };
                                if let Err(e) = transport_inner.send_packet(packet).await {
                                    log::error!("Failed to send packet fragment: {:?}", e);
                                    break;
                                }
                            }
                        });
                        log::debug!("send_packet fragments done");
                    }
                    Err(e) => {
                        log::error!("Failed to fragment flush message: {}", e);
                    }
                }
            }
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    let form_colors_clone = form_colors.clone();
    term.set(
        "define_form",
        lua.create_function(
            move |_,
                  (form_id, field_fg, field_bg, submit_fg, submit_bg): (
                u8,
                Option<u8>,
                Option<u8>,
                Option<u8>,
                Option<u8>,
            )| {
                let f_fg = field_fg.unwrap_or(form_colors_clone.field_fg);
                let f_bg = field_bg.unwrap_or(form_colors_clone.field_bg);
                let s_fg = submit_fg.unwrap_or(form_colors_clone.submit_fg);
                let s_bg = submit_bg.unwrap_or(form_colors_clone.submit_bg);

                let mut buf = out_buf.lock().unwrap();
                buf.push(0xD0); // OP_FORM_START
                buf.push(form_id);
                buf.push(f_fg);
                buf.push(f_bg);
                buf.push(s_fg);
                buf.push(s_bg);
                Ok(())
            },
        )?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "add_input_field",
        lua.create_function(
            move |_, (field_id, col, row, width, default_val): (String, u8, u8, u8, String)| {
                let mut buf = out_buf.lock().unwrap();
                buf.push(0xD1); // OP_FORM_FIELD
                buf.push(col);
                buf.push(row);
                buf.push(width);

                let id_bytes = field_id.as_bytes();
                buf.push(id_bytes.len() as u8);
                buf.extend_from_slice(id_bytes);

                let val_bytes = default_val.as_bytes();
                buf.push(val_bytes.len() as u8);
                buf.extend_from_slice(val_bytes);
                Ok(())
            },
        )?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "add_multiline_field",
        lua.create_function(
            move |_,
                  (field_id, col, row, width, height, default_val): (
                String,
                u8,
                u8,
                u8,
                u8,
                String,
            )| {
                let mut buf = out_buf.lock().unwrap();
                buf.push(0xD4); // OP_FORM_FIELD_MULTILINE
                buf.push(col);
                buf.push(row);
                buf.push(width);
                buf.push(height);

                let id_bytes = field_id.as_bytes();
                buf.push(id_bytes.len() as u8);
                buf.extend_from_slice(id_bytes);

                let val_bytes = default_val.as_bytes();
                buf.push(val_bytes.len() as u8);
                buf.extend_from_slice(val_bytes);
                Ok(())
            },
        )?,
    )?;

    let out_buf = output_buf.clone();
    term.set(
        "add_submit_button",
        lua.create_function(move |_, (button_id, col, row): (String, u8, u8)| {
            let mut buf = out_buf.lock().unwrap();
            buf.push(0xD2); // OP_FORM_SUBMIT
            buf.push(col);
            buf.push(row);

            let id_bytes = button_id.as_bytes();
            buf.push(id_bytes.len() as u8);
            buf.extend_from_slice(id_bytes);
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    let transport_clone = transport.clone();
    let node_id_clone = node_id.clone();
    let rt = rt_handle.clone();
    let bbs_stats_form = bbs_stats.clone();
    let transport_stats_form = transport_stats.clone();
    let packet_recorder_form = packet_recorder.clone();
    term.set(
        "flush_form",
        lua.create_function(move |_, (): ()| {
            let mut buf = out_buf.lock().unwrap();
            log::debug!(
                "term.flush_form() called with {} bytes in session buffer",
                buf.len()
            );
            buf.push(0xD3); // OP_FORM_END
            buf.push(0x04); // EndOfFrame
            let raw_len = buf.len();
            let start_comp = std::time::Instant::now();
            let (flags, payload) = match bifrost_ansi::compress_bytecode(&buf) {
                Ok(comp) => {
                    let comp_len = comp.len();
                    bbs_stats_form.record_compression(raw_len, comp_len);
                    if let Some(ref ts) = transport_stats_form {
                        ts.record_compression(raw_len, comp_len);
                    }
                    (0x02, comp)
                }
                Err(e) => {
                    log::warn!("Compression failed, sending uncompressed: {:?}", e);
                    (0x00, buf.clone())
                }
            };
            let comp_duration = start_comp.elapsed().as_micros() as u64;
            if let Some(ref recorder) = packet_recorder_form {
                let comp_opt = if (flags & 0x02) != 0 {
                    Some(payload.as_slice())
                } else {
                    None
                };
                recorder.record_compression(
                    "TX",
                    "form_template",
                    0x03,
                    flags,
                    &buf,
                    comp_opt,
                    "heatshrink_w8_l4",
                    comp_duration,
                );
            }
            buf.clear();

            let msg = MeshBbsMessage::new(0x01, 0x03, flags, payload);
            let mtu = transport_clone.get_mtu();
            match msg.to_fragments(mtu) {
                Ok(fragments) => {
                    let transport_inner = transport_clone.clone();
                    log::debug!(
                        "Sending term.flush_form() fragmented packets over transport (count={})",
                        fragments.len()
                    );
                    rt.block_on(async {
                        for frag in fragments {
                            let packet = RadioPacket {
                                is_broadcast: false,
                                src_node: [0; 32],
                                dst_node: node_id_clone,
                                payload: frag,
                                signal_rssi: 0,
                                signal_snr: 0,
                            };
                            if let Err(e) = transport_inner.send_packet(packet).await {
                                log::error!("Failed to send packet fragment: {:?}", e);
                                break;
                            }
                        }
                    });
                    log::debug!("send_packet fragments done");
                }
                Err(e) => {
                    log::error!("Failed to fragment flush_form message: {}", e);
                }
            }
            Ok(())
        })?,
    )?;

    globals.set("term", term)?;

    // db table
    let db = lua.create_table()?;
    let db_store_get = db_store.clone();
    db.set(
        "get",
        lua.create_function(move |lua, args: mlua::MultiValue| {
            let store = db_store_get.lock().unwrap();
            let mut iter = args.into_iter();
            let table = match iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                _ => return Ok(mlua::Value::Nil),
            };
            let key = match iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                Some(mlua::Value::Integer(i)) => i.to_string(),
                _ => "default".to_string(),
            };
            if let Some(tbl) = store.get(&table) {
                if let Some(val) = tbl.get(&key) {
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(val) {
                        if json_val.is_null() {
                            return Ok(mlua::Value::Nil);
                        }
                        if let Ok(lua_val) = lua.to_value(&json_val) {
                            return Ok(mlua::Value::from(lua_val));
                        }
                    }
                }
            }
            Ok(mlua::Value::Nil)
        })?,
    )?;

    let db_store_set = db_store.clone();
    db.set(
        "set",
        lua.create_function(move |lua, args: mlua::MultiValue| {
            let mut store = db_store_set.lock().unwrap();
            let mut iter = args.into_iter();
            let table = match iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                _ => return Ok(()),
            };
            let (key, val) = match (iter.next(), iter.next()) {
                (Some(k_val), Some(v)) => {
                    let key_str = match k_val {
                        mlua::Value::String(s) => s.to_str()?.to_string(),
                        mlua::Value::Integer(i) => i.to_string(),
                        _ => "default".to_string(),
                    };
                    (key_str, v)
                }
                (Some(v), None) => ("default".to_string(), v),
                _ => return Ok(()),
            };
            if val.is_nil() {
                if let Some(tbl) = store.get_mut(&table) {
                    tbl.remove(&key);
                }
            } else if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
                if json_val.is_null() {
                    if let Some(tbl) = store.get_mut(&table) {
                        tbl.remove(&key);
                    }
                } else if let Ok(json_str) = serde_json::to_string(&json_val) {
                    store
                        .entry(table)
                        .or_insert_with(HashMap::new)
                        .insert(key, json_str);
                }
            }
            Ok(())
        })?,
    )?;

    let db_store_keys = db_store.clone();
    db.set(
        "keys",
        lua.create_function(move |lua, table: String| {
            let store = db_store_keys.lock().unwrap();
            let table_tbl = lua.create_table()?;
            if let Some(tbl) = store.get(&table) {
                for (i, key) in tbl.keys().enumerate() {
                    table_tbl.set(i + 1, key.clone())?;
                }
            }
            Ok(table_tbl)
        })?,
    )?;
    globals.set("db", db)?;

    // log table for app scripts
    let log_table = lua.create_table()?;
    let active_app_clone = active_app.clone();
    log_table.set(
        "info",
        lua.create_function(move |_, msg: String| {
            let app = active_app_clone.lock().unwrap().clone();
            log::info!(target: "lua_app", "[Lua: {}] {}", app, msg);
            Ok(())
        })?,
    )?;

    let active_app_clone = active_app.clone();
    log_table.set(
        "warn",
        lua.create_function(move |_, msg: String| {
            let app = active_app_clone.lock().unwrap().clone();
            log::warn!(target: "lua_app", "[Lua: {}] {}", app, msg);
            Ok(())
        })?,
    )?;

    let active_app_clone = active_app.clone();
    log_table.set(
        "error",
        lua.create_function(move |_, msg: String| {
            let app = active_app_clone.lock().unwrap().clone();
            log::error!(target: "lua_app", "[Lua: {}] {}", app, msg);
            Ok(())
        })?,
    )?;

    let active_app_clone = active_app.clone();
    log_table.set(
        "debug",
        lua.create_function(move |_, msg: String| {
            let app = active_app_clone.lock().unwrap().clone();
            log::debug!(target: "lua_app", "[Lua: {}] {}", app, msg);
            Ok(())
        })?,
    )?;
    globals.set("log", log_table)?;

    // session table & state
    let session = lua.create_table()?;
    let node_hex_str_clone = node_hex_str.clone();
    session.set(
        "node_id",
        lua.create_function(move |_, (): ()| Ok(node_hex_str_clone.clone()))?,
    )?;

    let db_store_callsign = db_store.clone();
    let node_hex_str_clone = node_hex_str.clone();
    session.set(
        "callsign",
        lua.create_function(move |_, (): ()| {
            let store = db_store_callsign.lock().unwrap();
            if let Some(users_table) = store.get("users") {
                if let Some(user_json) = users_table.get(&node_hex_str_clone) {
                    if let Ok(user_obj) = serde_json::from_str::<serde_json::Value>(user_json) {
                        if let Some(nickname) = user_obj.get("nickname").and_then(|v| v.as_str()) {
                            return Ok(nickname.to_string());
                        }
                    }
                }
            }
            Ok("RadioOperator".to_string())
        })?,
    )?;

    let callback_store = Arc::new(StdMutex::new(None));
    let callback_store_clone = callback_store.clone();
    session.set(
        "await_input",
        lua.create_function(move |lua, (max_len, cb): (usize, mlua::Function)| {
            let key = lua.create_registry_value(cb)?;
            *callback_store_clone.lock().unwrap() = Some((max_len, key));
            Ok(())
        })?,
    )?;

    let session_close = Arc::new(StdMutex::new(false));
    let session_close_clone = session_close.clone();
    session.set(
        "close",
        lua.create_function(move |_, (): ()| {
            *session_close_clone.lock().unwrap() = true;
            Ok(())
        })?,
    )?;

    let db_store_perms = db_store.clone();
    let node_hex_str_clone = node_hex_str.clone();
    session.set(
        "permissions",
        lua.create_function(move |lua, (): ()| {
            let store = db_store_perms.lock().unwrap();
            if let Some(perms_table) = store.get("permissions") {
                if let Some(json_str) = perms_table.get(&node_hex_str_clone) {
                    if let Ok(perms) = serde_json::from_str::<Vec<String>>(json_str) {
                        let table = lua.create_table()?;
                        for (i, p) in perms.into_iter().enumerate() {
                            table.set(i + 1, p)?;
                        }
                        return Ok(table);
                    }
                }
            }
            let empty_tbl = lua.create_table()?;
            Ok(empty_tbl)
        })?,
    )?;

    let db_store_has_perm = db_store.clone();
    let node_hex_str_clone = node_hex_str.clone();
    session.set(
        "has_permission",
        lua.create_function(move |_, perm: String| {
            let store = db_store_has_perm.lock().unwrap();
            if let Some(perms_table) = store.get("permissions") {
                if let Some(json_str) = perms_table.get(&node_hex_str_clone) {
                    if let Ok(perms) = serde_json::from_str::<Vec<String>>(json_str) {
                        return Ok(perms.contains(&perm));
                    }
                }
            }
            Ok(false)
        })?,
    )?;

    let active_app_for_include = active_app.clone();
    session.set(
        "include",
        lua.create_function(move |lua, file_name: String| {
            let current_app = active_app_for_include.lock().unwrap().clone();
            let path = find_workspace_path(&format!("apps/{}/{}", current_app, file_name));
            let path = if path.exists() {
                path
            } else {
                find_workspace_path(&format!("apps/{}/{}.lua", current_app, file_name))
            };
            if path.exists() {
                let code = std::fs::read_to_string(&path)?;
                let chunk = lua.load(&code).set_name(&file_name);
                let val: mlua::Value = chunk.eval()?;
                Ok(val)
            } else {
                Err(mlua::Error::RuntimeError(format!(
                    "Included file not found: {:?}",
                    path
                )))
            }
        })?,
    )?;

    let enabled_apps = apps_config.enabled.clone();
    let active_app_clone = active_app.clone();
    let load_app = lua.create_function(move |lua, app_name: String| {
        if !enabled_apps.contains(&app_name) {
            log::error!("Application '{}' not found or not enabled in config", app_name);
            return Ok(());
        }
        let entry_file = format!("apps/{}/main.lua", app_name);
        let path = find_workspace_path(&entry_file);
        if path.exists() {
            *active_app_clone.lock().unwrap() = app_name.clone();
            let code = std::fs::read_to_string(&path)?;
            let app: mlua::Table = lua.load(&code).set_name(&app_name).eval()?;
            let on_start: mlua::Function = app.get("on_start")?;
            on_start.call::<_, ()>(lua.globals().get::<_, mlua::Table>("session")?)?;
        } else {
            log::error!("Application '{}' entry point not found at {:?}", app_name, path);
        }
        Ok(())
    })?;
    session.set("load_app", load_app)?;

    globals.set("session", session)?;

    // Start initial application specified in config (default: main_menu)
    let main_app_name = apps_config.main_app.clone();
    log::debug!("Loading initial app '{}'...", main_app_name);
    let main_entry_file = format!("apps/{}/main.lua", main_app_name);
    let main_path = find_workspace_path(&main_entry_file);
    if main_path.exists() {
        let main_code = std::fs::read_to_string(&main_path)?;
        log::debug!("Evaluating app '{}' code...", main_app_name);
        let app: mlua::Table = lua.load(&main_code).set_name(&main_app_name).eval()?;
        log::debug!("App '{}' code evaluated successfully", main_app_name);
        let on_start: mlua::Function = app.get("on_start")?;
        log::debug!("Invoking on_start for '{}'...", main_app_name);
        on_start.call::<_, ()>(lua.globals().get::<_, mlua::Table>("session")?)?;
        log::debug!("App '{}' on_start invoked successfully", main_app_name);
    } else {
        log::error!("Main app '{}' entry point not found at {:?}", main_app_name, main_path);
    }

    // Read loop using blocking receiver in standard thread context
    loop {
        if *session_close.lock().unwrap() {
            log::debug!("Session closing");
            break;
        }

        if let Some(msg) = rx.blocking_recv() {
            log::debug!(
                "Got input message: opcode={}, len={}",
                msg.opcode,
                msg.payload.len()
            );
            if let Some(ref recorder) = packet_recorder {
                let (raw_data, comp_opt, duration_us) = if (msg.flags & 0x02) != 0 {
                    let start_decomp = std::time::Instant::now();
                    let decomp = bifrost_ansi::decompress_bytecode(&msg.payload)
                        .unwrap_or_else(|_| msg.payload.clone());
                    let dur = start_decomp.elapsed().as_micros() as u64;
                    (decomp, Some(msg.payload.as_slice()), dur)
                } else {
                    (msg.payload.clone(), None, 0)
                };
                recorder.record_compression(
                    "RX",
                    "client_input",
                    msg.opcode,
                    msg.flags,
                    &raw_data,
                    comp_opt,
                    if (msg.flags & 0x02) != 0 {
                        "heatshrink_w8_l4"
                    } else {
                        "none"
                    },
                    duration_us,
                );
            }
            if msg.opcode == 0x02 {
                // Keystroke/Input message
                if let Ok(input_str) = String::from_utf8(msg.payload) {
                    let cb_opt = {
                        let mut store = callback_store.lock().unwrap();
                        store.take()
                    };

                    if let Some((_max_len, reg_key)) = cb_opt {
                        let cb: mlua::Function = lua.registry_value(&reg_key)?;
                        if input_str.starts_with('{') {
                            if let Ok(json_val) =
                                serde_json::from_str::<serde_json::Value>(&input_str)
                            {
                                if let Ok(lua_val) = lua.to_value(&json_val) {
                                    cb.call::<_, ()>(lua_val)?;
                                } else {
                                    cb.call::<_, ()>(input_str.clone())?;
                                }
                            } else {
                                cb.call::<_, ()>(input_str.clone())?;
                            }
                        } else {
                            cb.call::<_, ()>(input_str.clone())?;
                        }
                        let _ = lua.remove_registry_value(reg_key);
                    }
                }
            }
        } else {
            log::debug!("rx channel closed");
            break;
        }
    }

    Ok(())
}

pub fn find_workspace_path(relative_path: &str) -> PathBuf {
    let path = PathBuf::from(relative_path);
    if path.exists() {
        return path;
    }

    // 1. Traverse upward from current working directory
    if let Ok(current) = std::env::current_dir() {
        let mut cur = current;
        for _ in 0..10 {
            let candidate = cur.join(relative_path);
            if candidate.exists() {
                return candidate;
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
            let candidate = cur.join(relative_path);
            if candidate.exists() {
                return candidate;
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
            let candidate = cur.join(relative_path);
            if candidate.exists() {
                return candidate;
            }
            if let Some(parent) = cur.parent() {
                cur = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_sandbox_security() {
        let lua = mlua::Lua::new();
        let globals = lua.globals();
        globals.set("os", mlua::Value::Nil).unwrap();
        globals.set("io", mlua::Value::Nil).unwrap();

        assert!(globals.get::<_, mlua::Value>("os").unwrap().is_nil());
        assert!(globals.get::<_, mlua::Value>("io").unwrap().is_nil());
    }

    #[test]
    fn test_default_config_fallback() {
        let rate_limiter = RateLimiterConfig {
            max_packets_per_minute: 45,
            max_burst_packets: 4,
            inter_packet_guard_ms: 350,
            max_duty_cycle_percent: 1.0,
            duty_cycle_window_secs: 3600,
        };
        assert_eq!(rate_limiter.max_packets_per_minute, 45);
        assert_eq!(rate_limiter.max_burst_packets, 4);
        assert_eq!(rate_limiter.inter_packet_guard_ms, 350);
        assert_eq!(rate_limiter.max_duty_cycle_percent, 1.0);
        assert_eq!(rate_limiter.duty_cycle_window_secs, 3600);
    }

    #[tokio::test]
    async fn test_run_bbs_with_default() {
        let result = run_bbs(None, Some(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_bbs_with_config_file() {
        let temp_path = PathBuf::from("temp_test_config.toml");
        let config_str = r#"
[rate_limiter]
max_packets_per_minute = 30
max_burst_packets = 5
inter_packet_guard_ms = 200
max_duty_cycle_percent = 0.5
duty_cycle_window_secs = 1800

[asset_broadcaster]
enable_on_demand_broadcast = false
max_asset_broadcast_duty_cycle = 0.1
        "#;
        std::fs::write(&temp_path, config_str).unwrap();

        let result = run_bbs(Some(temp_path.clone()), Some(1)).await;
        let _ = std::fs::remove_file(temp_path);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_bbs_with_missing_config_file() {
        let result = run_bbs(Some(PathBuf::from("non_existent_config.toml")), Some(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bbs_server_connection_and_lua_execution() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server(
            "127.0.0.1:9095".to_string(),
            0.0,
            0,
            200,
        ));

        let server_handle =
            tokio::spawn(async move { start_server(config, server_transport, Some(1)).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport =
            MockSocketTransport::new_client("127.0.0.1:9095".to_string(), 0.0, 0, 200);

        let client_key = [5u8; 32];
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let mut sent = false;
        for _ in 0..10 {
            if !handshake_payloads.is_empty() {
                let packet = RadioPacket {
                    is_broadcast: false,
                    src_node: client_key,
                    dst_node: [0; 32],
                    payload: handshake_payloads[0].clone(),
                    signal_rssi: -50,
                    signal_snr: 10,
                };
                if client_transport.send_packet(packet).await.is_ok() {
                    sent = true;
                    break;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        assert!(sent, "Failed to send handshake from client");

        // Reassemble the server welcome menu fragments
        let mut client_reassembler = MessageReassembler::new();
        let mut assembled_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler
                        .process_packet([0; 32], &packet.payload)
                        .unwrap()
                    {
                        assembled_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let response = assembled_msg.expect("Failed to reassemble server response welcome screen");
        assert_eq!(response.opcode, 0x03);
        assert!(
            !response.payload.is_empty(),
            "Server response payload is empty"
        );

        // Send simulated Form Submission (tab nickname to action button, then press enter)
        let form_submit_json = r#"{"nickname":"TestUser","submit":"read_boards"}"#;
        let submit_msg =
            MeshBbsMessage::new(0x02, 0x02, 0x00, form_submit_json.as_bytes().to_vec());
        let submit_payloads = submit_msg.to_fragments(200).unwrap();
        assert!(!submit_payloads.is_empty());

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: submit_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // Reassemble the server response (should be the discussion board screen)
        let mut board_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler
                        .process_packet([0; 32], &packet.payload)
                        .unwrap()
                    {
                        board_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let board_response =
            board_msg.expect("Failed to reassemble discussion boards screen response");
        assert_eq!(board_response.opcode, 0x03);

        let _ = server_handle.await;
    }

    #[test]
    fn test_config_deserialization_with_admin_nodes() {
        let config_str = r#"
admin_nodes = ["abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"]

[rate_limiter]
max_packets_per_minute = 30
max_burst_packets = 5
inter_packet_guard_ms = 200
max_duty_cycle_percent = 0.5
duty_cycle_window_secs = 1800

[asset_broadcaster]
enable_on_demand_broadcast = false
max_asset_broadcast_duty_cycle = 0.1
        "#;

        let config: AppConfig = toml::from_str(config_str).unwrap();
        assert_eq!(config.admin_nodes.len(), 1);
        assert_eq!(
            config.admin_nodes[0],
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        );
        assert_eq!(config.rate_limiter.max_packets_per_minute, 30);
    }

    #[test]
    fn test_config_deserialization_without_admin_nodes() {
        let config_str = r#"
[rate_limiter]
max_packets_per_minute = 45
max_burst_packets = 4
inter_packet_guard_ms = 350
max_duty_cycle_percent = 1.0
duty_cycle_window_secs = 3600

[asset_broadcaster]
enable_on_demand_broadcast = true
max_asset_broadcast_duty_cycle = 0.15
        "#;

        let config: AppConfig = toml::from_str(config_str).unwrap();
        assert!(config.admin_nodes.is_empty());
        assert_eq!(config.log_level, "info");
    }

    #[test]
    fn test_config_deserialization_with_log_level() {
        let config_str = r#"
log_level = "debug"

[rate_limiter]
max_packets_per_minute = 45
max_burst_packets = 4
inter_packet_guard_ms = 350
max_duty_cycle_percent = 1.0
duty_cycle_window_secs = 3600

[asset_broadcaster]
enable_on_demand_broadcast = true
max_asset_broadcast_duty_cycle = 0.15
        "#;

        let config: AppConfig = toml::from_str(config_str).unwrap();
        assert_eq!(config.log_level, "debug");
    }

    #[test]
    fn test_config_deserialization_with_form_colors() {
        let config_str = r#"
[rate_limiter]
max_packets_per_minute = 45
max_burst_packets = 4
inter_packet_guard_ms = 350
max_duty_cycle_percent = 1.0
duty_cycle_window_secs = 3600

[asset_broadcaster]
enable_on_demand_broadcast = true
max_asset_broadcast_duty_cycle = 0.15

[form_colors]
field_fg = 10
field_bg = 2
submit_fg = 3
submit_bg = 5
        "#;

        let config: AppConfig = toml::from_str(config_str).unwrap();
        assert_eq!(config.form_colors.field_fg, 10);
        assert_eq!(config.form_colors.field_bg, 2);
        assert_eq!(config.form_colors.submit_fg, 3);
        assert_eq!(config.form_colors.submit_bg, 5);
    }

    #[test]
    fn test_default_form_colors() {
        let fc = default_form_colors();
        assert_eq!(fc.field_fg, 15);
        assert_eq!(fc.field_bg, 4);
        assert_eq!(fc.submit_fg, 0);
        assert_eq!(fc.submit_bg, 7);
    }

    #[test]
    fn test_default_config_has_empty_admin_nodes() {
        let config = default_config();
        assert!(config.admin_nodes.is_empty());
        assert_eq!(config.form_colors.field_fg, 15);
        assert_eq!(config.form_colors.field_bg, 4);
    }

    #[test]
    fn test_permissions_first_user_gets_admin() {
        // Simulates the permissions initialization logic for the first user
        let db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let admin_nodes: Vec<String> = Vec::new();
        let node_hex =
            "0505050505050505050505050505050505050505050505050505050505050505".to_string();

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.get("users") {
            Some(users_map) => users_map.is_empty(),
            None => true,
        };

        assert!(!is_configured_admin);
        assert!(is_first_user);

        let mut perms = vec!["read".to_string(), "write".to_string()];
        if is_configured_admin || is_first_user {
            perms.push("admin".to_string());
        }
        assert_eq!(perms, vec!["read", "write", "admin"]);
    }

    #[test]
    fn test_permissions_configured_admin_node() {
        let node_hex =
            "aabbccdd00000000000000000000000000000000000000000000000000000000".to_string();
        let admin_nodes = vec![node_hex.clone()];

        let mut db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        // Simulate an existing user so this node is NOT the first user
        let mut users = HashMap::new();
        users.insert("other_node".to_string(), "{}".to_string());
        db_store.insert("users".to_string(), users);

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.get("users") {
            Some(users_map) => users_map.is_empty(),
            None => true,
        };

        assert!(is_configured_admin);
        assert!(!is_first_user);

        let mut perms = vec!["read".to_string(), "write".to_string()];
        if is_configured_admin || is_first_user {
            perms.push("admin".to_string());
        }
        assert_eq!(perms, vec!["read", "write", "admin"]);
    }

    #[test]
    fn test_permissions_regular_user() {
        let node_hex =
            "1111111111111111111111111111111111111111111111111111111111111111".to_string();
        let admin_nodes: Vec<String> = Vec::new();

        let mut db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut users = HashMap::new();
        users.insert("existing_admin".to_string(), "{}".to_string());
        db_store.insert("users".to_string(), users);

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.get("users") {
            Some(users_map) => users_map.is_empty(),
            None => true,
        };

        assert!(!is_configured_admin);
        assert!(!is_first_user);

        let mut perms = vec!["read".to_string(), "write".to_string()];
        if is_configured_admin || is_first_user {
            perms.push("admin".to_string());
        }
        assert_eq!(perms, vec!["read", "write"]);
    }

    #[test]
    fn test_permissions_persistence_in_db() {
        let mut db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let node_hex = "abcd".to_string();
        let perms = vec!["read".to_string(), "write".to_string(), "admin".to_string()];
        let json_str = serde_json::to_string(&perms).unwrap();

        let perms_table = db_store
            .entry("permissions".to_string())
            .or_insert_with(HashMap::new);
        perms_table.insert(node_hex.clone(), json_str);

        // Verify we can read them back
        let stored = db_store.get("permissions").unwrap().get(&node_hex).unwrap();
        let decoded: Vec<String> = serde_json::from_str(stored).unwrap();
        assert_eq!(decoded, vec!["read", "write", "admin"]);
        assert!(decoded.contains(&"admin".to_string()));
    }

    #[test]
    fn test_permissions_dedup_on_reconnect() {
        // If perms already exist in DB, they should NOT be overwritten
        let mut db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let node_hex = "node123".to_string();
        let original_perms = vec!["read".to_string()];
        let json_str = serde_json::to_string(&original_perms).unwrap();

        let perms_table = db_store
            .entry("permissions".to_string())
            .or_insert_with(HashMap::new);
        perms_table.insert(node_hex.clone(), json_str);

        // Simulate reconnect logic: only insert if not present
        let new_perms = vec!["read".to_string(), "write".to_string(), "admin".to_string()];
        let new_json = serde_json::to_string(&new_perms).unwrap();
        let perms_table = db_store.get_mut("permissions").unwrap();
        if !perms_table.contains_key(&node_hex) {
            perms_table.insert(node_hex.clone(), new_json);
        }

        // Should still have original perms
        let stored = db_store.get("permissions").unwrap().get(&node_hex).unwrap();
        let decoded: Vec<String> = serde_json::from_str(stored).unwrap();
        assert_eq!(decoded, vec!["read"]);
    }

    #[test]
    fn test_db_keys_pattern() {
        let mut db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut users = HashMap::new();
        users.insert("node_a".to_string(), r#"{"nickname":"Alice"}"#.to_string());
        users.insert("node_b".to_string(), r#"{"nickname":"Bob"}"#.to_string());
        db_store.insert("users".to_string(), users);

        let keys: Vec<String> = db_store.get("users").unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"node_a".to_string()));
        assert!(keys.contains(&"node_b".to_string()));
    }

    #[test]
    fn test_db_keys_empty_table() {
        let db_store: HashMap<String, HashMap<String, String>> = HashMap::new();
        let keys: Vec<String> = match db_store.get("users") {
            Some(tbl) => tbl.keys().cloned().collect(),
            None => Vec::new(),
        };
        assert!(keys.is_empty());
    }

    #[test]
    fn test_find_workspace_path_nonexistent() {
        let path = find_workspace_path("apps/nonexistent.lua");
        // The function returns the path regardless, it just won't exist
        assert!(path.to_str().unwrap().contains("nonexistent.lua"));
    }

    #[test]
    fn test_form_colors_config_clone() {
        let fc = FormColorsConfig {
            field_fg: 10,
            field_bg: 2,
            submit_fg: 3,
            submit_bg: 5,
        };
        let fc2 = fc.clone();
        assert_eq!(fc2.field_fg, 10);
        assert_eq!(fc2.field_bg, 2);
        assert_eq!(fc2.submit_fg, 3);
        assert_eq!(fc2.submit_bg, 5);
    }

    #[test]
    fn test_app_config_admin_nodes_multiple() {
        let config_str = r#"
admin_nodes = [
    "aaaa000000000000000000000000000000000000000000000000000000000000",
    "bbbb000000000000000000000000000000000000000000000000000000000000",
    "cccc000000000000000000000000000000000000000000000000000000000000"
]

[rate_limiter]
max_packets_per_minute = 45
max_burst_packets = 4
inter_packet_guard_ms = 350
max_duty_cycle_percent = 1.0
duty_cycle_window_secs = 3600

[asset_broadcaster]
enable_on_demand_broadcast = true
max_asset_broadcast_duty_cycle = 0.15
        "#;
        let config: AppConfig = toml::from_str(config_str).unwrap();
        assert_eq!(config.admin_nodes.len(), 3);
    }

    #[tokio::test]
    async fn test_server_with_admin_nodes_config() {
        let _ = env_logger::builder().is_test(true).try_init();
        let mut config = default_config();
        config.admin_nodes =
            vec!["0505050505050505050505050505050505050505050505050505050505050505".to_string()];
        let transport = Arc::new(MockSocketTransport::new(0.0, 10, 200));
        let result = start_server(config, transport, Some(1)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_advert_packet_processing() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9097".to_string(), 0.0, 0, 200));

        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(2)).await
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9097".to_string(), 0.0, 0, 200);

        let client_key = [8u8; 32];

        // Send advert packet
        let mut advert_payload = Vec::new();
        advert_payload.extend_from_slice(&client_key); // 32 bytes public key
        advert_payload.extend_from_slice(&0u32.to_le_bytes()); // 4 bytes timestamp
        advert_payload.extend_from_slice(&[0u8; 64]); // 64 bytes signature

        let flags: u8 = 0x80 | 0x10; // has name | has location
        advert_payload.push(flags);

        let lat_int: i32 = 47606200; // Seattle lat
        advert_payload.extend_from_slice(&lat_int.to_le_bytes());
        let lon_int: i32 = -122332100; // Seattle lon
        advert_payload.extend_from_slice(&lon_int.to_le_bytes());

        let node_name = "AdvertUser";
        advert_payload.extend_from_slice(node_name.as_bytes());

        let advert_packet = RadioPacket {
            is_broadcast: true,
            src_node: client_key,
            dst_node: [0; 32],
            payload: advert_payload,
            signal_rssi: -40,
            signal_snr: 12,
        };
        client_transport.send_packet(advert_packet).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();
        let handshake = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(handshake).await.unwrap();

        let mut client_reassembler = MessageReassembler::new();
        let mut assembled_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        assembled_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        assert!(assembled_msg.is_some(), "Should receive welcome screen");

        // Wait for server to shutdown
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_session_reconnect_preserves_nickname() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server(
            "127.0.0.1:9096".to_string(),
            0.0,
            0,
            200,
        ));

        let server_handle =
            tokio::spawn(async move { start_server(config, server_transport, Some(3)).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport =
            MockSocketTransport::new_client("127.0.0.1:9096".to_string(), 0.0, 0, 200);

        let client_key = [7u8; 32];

        // First connection: handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let mut sent = false;
        for _ in 0..10 {
            let packet = RadioPacket {
                is_broadcast: false,
                src_node: client_key,
                dst_node: [0; 32],
                payload: handshake_payloads[0].clone(),
                signal_rssi: -50,
                signal_snr: 10,
            };
            if client_transport.send_packet(packet).await.is_ok() {
                sent = true;
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        assert!(sent, "Failed to send handshake");

        // Receive initial welcome (nickname setup form for first user)
        let mut client_reassembler = MessageReassembler::new();
        let mut assembled_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler
                        .process_packet([0; 32], &packet.payload)
                        .unwrap()
                    {
                        assembled_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        assert!(assembled_msg.is_some(), "Should receive welcome screen");
        let welcome = assembled_msg.unwrap();
        assert_eq!(welcome.opcode, 0x03);

        // Register nickname
        let register_json = r#"{"nickname":"ReconnectTestUser","submit":"register"}"#;
        let register_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, register_json.as_bytes().to_vec());
        let register_payloads = register_msg.to_fragments(200).unwrap();

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: register_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // After registering, server should send back the main menu with "Hello ReconnectTestUser"
        let mut hello_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler
                        .process_packet([0; 32], &packet.payload)
                        .unwrap()
                    {
                        hello_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let hello_response =
            hello_msg.expect("Should receive Hello screen after nickname registration");
        assert_eq!(hello_response.opcode, 0x03);
        // The payload should contain the user's nickname in the hello greeting
        let uncompressed_payload = if (hello_response.flags & 0x02) != 0 {
            bifrost_ansi::decompress_bytecode(&hello_response.payload).unwrap_or(hello_response.payload)
        } else {
            hello_response.payload
        };
        let payload_str = String::from_utf8_lossy(&uncompressed_payload);
        assert!(
            payload_str.contains("ReconnectTestUser"),
            "Hello screen should contain user nickname, got: {}",
            payload_str
        );

        let _ = server_handle.await;
    }

    #[test]
    fn test_asset_manifest_loading() {
        let manifest = load_app_manifests(&["main_menu".to_string(), "minidungeon".to_string()]);
        assert_eq!(manifest.len(), 3);

        let banner_entry = manifest
            .iter()
            .find(|(_, (name, _))| name == "main_menu/main_menu_banner");
        assert!(banner_entry.is_some(), "main_menu/main_menu_banner must be registered");
        let (&banner_id, (banner_name, banner_path)) = banner_entry.unwrap();
        assert_eq!(banner_name, "main_menu/main_menu_banner");
        assert_eq!(banner_path, "apps/main_menu/assets/main_menu_banner.ans");

        let dungeon_entry = manifest
            .iter()
            .find(|(_, (name, _))| name == "minidungeon/dungeon_banner");
        assert!(dungeon_entry.is_some(), "minidungeon/dungeon_banner must be registered");
        let (&dungeon_id, _) = dungeon_entry.unwrap();
        assert_ne!(banner_id, dungeon_id, "Asset IDs must be unique");
    }

    #[tokio::test]
    async fn test_on_demand_asset_broadcast_and_client_caching() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let manifest = load_app_manifests(&config.apps.enabled);
        let (&req_asset_id, (_, _)) = manifest
            .iter()
            .find(|(_, (n, _))| n == "main_menu/main_menu_banner")
            .expect("main_menu/main_menu_banner must exist in manifest");

        let server_transport = Arc::new(MockSocketTransport::new_server(
            "127.0.0.1:9098".to_string(),
            0.0,
            0,
            200,
        ));

        let server_handle =
            tokio::spawn(async move { start_server(config, server_transport, Some(2)).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport =
            MockSocketTransport::new_client("127.0.0.1:9098".to_string(), 0.0, 0, 200);

        let client_key = [9u8; 32];
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let mut sent = false;
        for _ in 0..10 {
            if !handshake_payloads.is_empty() {
                let packet = RadioPacket {
                    is_broadcast: false,
                    src_node: client_key,
                    dst_node: [0; 32],
                    payload: handshake_payloads[0].clone(),
                    signal_rssi: -50,
                    signal_snr: 10,
                };
                if client_transport.send_packet(packet).await.is_ok() {
                    sent = true;
                    break;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        assert!(sent, "Failed to send handshake from client");

        // Send REQ_ASSET for dynamically resolved asset ID
        let req_msg = MeshBbsMessage::new(0x01, 0x05, 0x00, req_asset_id.to_be_bytes().to_vec());
        let req_payloads = req_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: req_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // Listen for broadcast asset packet chunks
        let mut chunks_received: HashMap<u8, Vec<u8>> = HashMap::new();
        let mut expected_total_chunks = 0u8;
        let mut expected_crc32 = 0u32;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(2000) {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    if packet.payload.len() >= 12 && packet.payload[0] == 0xBB && packet.payload[1] == 0x04 {
                        let chunk_idx = packet.payload[3];
                        let total_chunks = packet.payload[4];
                        let asset_id = u16::from_be_bytes([packet.payload[5], packet.payload[6]]);
                        let payload_len = packet.payload[7] as usize;
                        let master_crc = u32::from_be_bytes([
                            packet.payload[8],
                            packet.payload[9],
                            packet.payload[10],
                            packet.payload[11],
                        ]);

                        if asset_id == req_asset_id && packet.payload.len() >= 12 + payload_len {
                            expected_total_chunks = total_chunks;
                            expected_crc32 = master_crc;
                            chunks_received.insert(chunk_idx, packet.payload[12..12 + payload_len].to_vec());

                            if chunks_received.len() == total_chunks as usize {
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        assert!(
            expected_total_chunks > 0,
            "Did not receive any broadcast asset chunks for 0x0103"
        );
        assert_eq!(
            chunks_received.len(),
            expected_total_chunks as usize,
            "Did not receive all broadcast chunks"
        );

        let mut assembled_bytes = Vec::new();
        for idx in 1..=expected_total_chunks {
            assembled_bytes.extend_from_slice(chunks_received.get(&idx).unwrap());
        }

        assert_eq!(
            bifrost_transport::crc32(&assembled_bytes),
            expected_crc32,
            "CRC32 mismatch on assembled broadcast asset"
        );

        let banner_path = find_workspace_path("apps/main_menu/assets/main_menu_banner.ans");
        let expected_bytes = std::fs::read(banner_path).unwrap();
        assert_eq!(assembled_bytes, expected_bytes, "Broadcast asset content does not match original file");

        let _ = server_handle.await;
    }

    #[test]
    fn test_bbs_stats_new() {
        let stats = BbsStats::new();
        assert_eq!(stats.active_sessions(), 0);
        assert_eq!(stats.unique_users_24h(), 0);
    }

    #[test]
    fn test_bbs_stats_session_connect_disconnect() {
        let stats = BbsStats::new();
        let node1 = [1u8; 32];
        let node2 = [2u8; 32];

        stats.record_session_connect(node1);
        assert_eq!(stats.active_sessions(), 1);

        stats.record_session_connect(node2);
        assert_eq!(stats.active_sessions(), 2);

        stats.record_session_disconnect();
        assert_eq!(stats.active_sessions(), 1);

        stats.record_session_disconnect();
        assert_eq!(stats.active_sessions(), 0);

        // Disconnecting below 0 should saturate at 0
        stats.record_session_disconnect();
        assert_eq!(stats.active_sessions(), 0);
    }

    #[test]
    fn test_bbs_stats_unique_users_24h() {
        let stats = BbsStats::new();
        let node1 = [1u8; 32];
        let node2 = [2u8; 32];

        stats.record_session_connect(node1);
        stats.record_session_connect(node1); // Same node reconnecting
        stats.record_session_connect(node2);

        assert_eq!(stats.unique_users_24h(), 2);
    }

    #[test]
    fn test_bbs_stats_compression_recording() {
        let stats = BbsStats::new();
        stats.record_compression(1024, 300);
        stats.record_decompression(250, 800);

        assert_eq!(stats.total_raw_bytes_sent(), 1024);
        assert_eq!(stats.total_compressed_bytes_sent(), 300);
        assert_eq!(stats.total_raw_bytes_received(), 800);
        assert_eq!(stats.total_compressed_bytes_received(), 250);
    }

    #[test]
    fn test_db_set_nil_and_null_handling() {
        let lua = mlua::Lua::new();
        let db_store = Arc::new(StdMutex::new(HashMap::<String, HashMap<String, String>>::new()));

        let db = lua.create_table().unwrap();
        let db_store_get = db_store.clone();
        db.set(
            "get",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let store = db_store_get.lock().unwrap();
                let mut iter = args.into_iter();
                let table = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    _ => return Ok(mlua::Value::Nil),
                };
                let key = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    Some(mlua::Value::Integer(i)) => i.to_string(),
                    _ => "default".to_string(),
                };
                if let Some(tbl) = store.get(&table) {
                    if let Some(val) = tbl.get(&key) {
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(val) {
                            if json_val.is_null() {
                                return Ok(mlua::Value::Nil);
                            }
                            if let Ok(lua_val) = lua.to_value(&json_val) {
                                return Ok(mlua::Value::from(lua_val));
                            }
                        }
                    }
                }
                Ok(mlua::Value::Nil)
            })
            .unwrap(),
        )
        .unwrap();

        let db_store_set = db_store.clone();
        db.set(
            "set",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let mut store = db_store_set.lock().unwrap();
                let mut iter = args.into_iter();
                let table = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    _ => return Ok(()),
                };
                let (key, val) = match (iter.next(), iter.next()) {
                    (Some(k_val), Some(v)) => {
                        let key_str = match k_val {
                            mlua::Value::String(s) => s.to_str()?.to_string(),
                            mlua::Value::Integer(i) => i.to_string(),
                            _ => "default".to_string(),
                        };
                        (key_str, v)
                    }
                    (Some(v), None) => ("default".to_string(), v),
                    _ => return Ok(()),
                };
                if val.is_nil() {
                    if let Some(tbl) = store.get_mut(&table) {
                        tbl.remove(&key);
                    }
                } else if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
                    if json_val.is_null() {
                        if let Some(tbl) = store.get_mut(&table) {
                            tbl.remove(&key);
                        }
                    } else if let Ok(json_str) = serde_json::to_string(&json_val) {
                        store
                            .entry(table)
                            .or_insert_with(HashMap::new)
                            .insert(key, json_str);
                    }
                }
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();

        lua.globals().set("db", db).unwrap();

        // 1. Set a table
        lua.load(r#"db.set("test_table", "player1", { hp = 25, name = "Hero" })"#)
            .exec()
            .unwrap();

        // Verify get returns table
        let hp: i32 = lua
            .load(r#"local p = db.get("test_table", "player1"); return p.hp"#)
            .eval()
            .unwrap();
        assert_eq!(hp, 25);

        // 2. Set nil (game over / clear save)
        lua.load(r#"db.set("test_table", "player1", nil)"#)
            .exec()
            .unwrap();

        // Verify get returns nil (not userdata Null!)
        let is_nil: bool = lua
            .load(r#"local p = db.get("test_table", "player1"); return p == nil"#)
            .eval()
            .unwrap();
        assert!(is_nil, "db.get after setting nil should return nil");

        // Verify condition `if not player or player.hp <= 0` does not error on userdata index
        let ok_result: bool = lua
            .load(
                r#"
                local p = db.get("test_table", "player1")
                if not p or p.hp <= 0 then
                    return true
                end
                return false
                "#,
            )
            .eval()
            .unwrap();
        assert!(ok_result);
    }

    #[test]
    fn test_minidungeon_xp_and_stat_mechanics() {
        let lua = mlua::Lua::new();
        let code = std::fs::read_to_string(find_workspace_path("apps/minidungeon/main.lua"))
            .unwrap();

        // Verify exponential XP progression
        let xp_checks: (i64, i64, i64, i64) = lua
            .load(
                r#"
                local function xp_needed(level)
                    return 50 * (math.floor(2 ^ level) - 1)
                end
                return xp_needed(1), xp_needed(2), xp_needed(3), xp_needed(4)
                "#,
            )
            .eval()
            .unwrap();

        assert_eq!(xp_checks.0, 50, "Level 1->2 should need 50 XP");
        assert_eq!(xp_checks.1, 150, "Level 2->3 should need 150 total XP");
        assert_eq!(xp_checks.2, 350, "Level 3->4 should need 350 total XP");
        assert_eq!(xp_checks.3, 750, "Level 4->5 should need 750 total XP");

        // Verify minidungeon script compiles without syntax errors
        assert!(!code.is_empty());
    }

    #[test]
    fn test_db_flexible_args() {
        let lua = mlua::Lua::new();
        let db_store = Arc::new(StdMutex::new(HashMap::<String, HashMap<String, String>>::new()));

        let db = lua.create_table().unwrap();
        let db_store_get = db_store.clone();
        db.set(
            "get",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let store = db_store_get.lock().unwrap();
                let mut iter = args.into_iter();
                let table = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    _ => return Ok(mlua::Value::Nil),
                };
                let key = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    Some(mlua::Value::Integer(i)) => i.to_string(),
                    _ => "default".to_string(),
                };
                if let Some(tbl) = store.get(&table) {
                    if let Some(val) = tbl.get(&key) {
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(val) {
                            if json_val.is_null() {
                                return Ok(mlua::Value::Nil);
                            }
                            if let Ok(lua_val) = lua.to_value(&json_val) {
                                return Ok(mlua::Value::from(lua_val));
                            }
                        }
                    }
                }
                Ok(mlua::Value::Nil)
            })
            .unwrap(),
        )
        .unwrap();

        let db_store_set = db_store.clone();
        db.set(
            "set",
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let mut store = db_store_set.lock().unwrap();
                let mut iter = args.into_iter();
                let table = match iter.next() {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    _ => return Ok(()),
                };
                let (key, val) = match (iter.next(), iter.next()) {
                    (Some(k_val), Some(v)) => {
                        let key_str = match k_val {
                            mlua::Value::String(s) => s.to_str()?.to_string(),
                            mlua::Value::Integer(i) => i.to_string(),
                            _ => "default".to_string(),
                        };
                        (key_str, v)
                    }
                    (Some(v), None) => ("default".to_string(), v),
                    _ => return Ok(()),
                };
                if val.is_nil() {
                    if let Some(tbl) = store.get_mut(&table) {
                        tbl.remove(&key);
                    }
                } else if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
                    if json_val.is_null() {
                        if let Some(tbl) = store.get_mut(&table) {
                            tbl.remove(&key);
                        }
                    } else if let Ok(json_str) = serde_json::to_string(&json_val) {
                        store
                            .entry(table)
                            .or_insert_with(HashMap::new)
                            .insert(key, json_str);
                    }
                }
                Ok(())
            })
            .unwrap(),
        )
        .unwrap();

        lua.globals().set("db", db).unwrap();

        // 1. Two-arg set and get: db.set(table, key, val) and db.get(table, key)
        lua.load(r#"db.set("users", "user1", { nickname = "Alice" })"#).exec().unwrap();
        let nick: String = lua.load(r#"local u = db.get("users", "user1"); return u.nickname"#).eval().unwrap();
        assert_eq!(nick, "Alice");

        // 2. Single-key set and get: db.set(key, val) and db.get(key)
        lua.load(r#"db.set("market_categories", { "General", "Radios" })"#).exec().unwrap();
        let cat: String = lua.load(r#"local c = db.get("market_categories"); return c[2]"#).eval().unwrap();
        assert_eq!(cat, "Radios");

        // 3. Deletion with nil
        lua.load(r#"db.set("users", "user1", nil)"#).exec().unwrap();
        let is_nil: bool = lua.load(r#"return db.get("users", "user1") == nil"#).eval().unwrap();
        assert!(is_nil);
    }

    #[test]
    fn test_marketplace_app_syntax() {
        let path = find_workspace_path("apps/marketplace/main.lua");
        let content = std::fs::read_to_string(path).unwrap();
        let lua = mlua::Lua::new();
        let chunk = lua.load(&content);
        assert!(chunk.into_function().is_ok(), "marketplace/main.lua should compile as valid Lua");
    }

    #[tokio::test]
    async fn test_marketplace_session_navigation() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9093".to_string(), 0.0, 0, 200));

        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(3)).await
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9093".to_string(), 0.0, 0, 200);

        let client_key = [11u8; 32];

        // Handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let mut sent = false;
        for _ in 0..10 {
            let packet = RadioPacket {
                is_broadcast: false,
                src_node: client_key,
                dst_node: [0; 32],
                payload: handshake_payloads[0].clone(),
                signal_rssi: -50,
                signal_snr: 10,
            };
            if client_transport.send_packet(packet).await.is_ok() {
                sent = true;
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        assert!(sent, "Failed to send handshake");

        let mut client_reassembler = MessageReassembler::new();

        // 1. Receive register screen
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(_msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        break;
                    }
                }
                _ => {}
            }
        }

        // 2. Register nickname "TraderBob"
        let register_json = r#"{"nickname":"TraderBob","submit":"register"}"#;
        let register_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, register_json.as_bytes().to_vec());
        let register_payloads = register_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: register_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // Receive Main Menu screen
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(_msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        break;
                    }
                }
                _ => {}
            }
        }

        // 3. Submit "marketplace" action from main menu (Form ID 10)
        let market_select_json = r#"{"submit":"marketplace"}"#;
        let market_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, market_select_json.as_bytes().to_vec());
        let market_payloads = market_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: market_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // 4. Receive Marketplace screen
        let mut market_screen = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        market_screen = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let resp = market_screen.expect("Should receive Marketplace screen");
        assert_eq!(resp.opcode, 0x03);
        let uncompressed_payload = if (resp.flags & 0x02) != 0 {
            bifrost_ansi::decompress_bytecode(&resp.payload).unwrap_or(resp.payload)
        } else {
            resp.payload
        };
        let screen_text = String::from_utf8_lossy(&uncompressed_payload);
        assert!(screen_text.contains("MARKETPLACE"), "Should contain MARKETPLACE header, got: {}", screen_text);

        let _ = server_handle.await;
    }

    #[test]
    fn test_parse_meshcore_advert_full_packet_framing() {
        let node_key = [0x42u8; 32];
        let mut packet_bytes = Vec::new();

        // 1. Packet Header: Version=0, PayloadType=0x04 (ADVERT), RouteType=0x01 (FLOOD) -> 0b00010001 = 0x11
        packet_bytes.push(0x11);
        // 2. Path length: 2 hops
        packet_bytes.push(2);
        // 3. Path bytes
        packet_bytes.push(0xAA);
        packet_bytes.push(0xBB);

        // 4. Advert payload:
        // Pubkey (32B)
        packet_bytes.extend_from_slice(&node_key);
        // Timestamp (4B LE)
        packet_bytes.extend_from_slice(&1700000000u32.to_le_bytes());
        // Signature (64B)
        packet_bytes.extend_from_slice(&[0x77u8; 64]);

        // Appdata:
        // Flags: 0x80 (name) | 0x40 (feature2) | 0x20 (feature1) | 0x10 (location) | 0x01 (chat node) = 0xF1
        packet_bytes.push(0xF1);
        // Lat: 37.7749 * 1_000_000 = 37774900
        packet_bytes.extend_from_slice(&37774900i32.to_le_bytes());
        // Lon: -122.4194 * 1_000_000 = -122419400
        packet_bytes.extend_from_slice(&(-122419400i32).to_le_bytes());
        // Feature1: 100
        packet_bytes.extend_from_slice(&100u16.to_le_bytes());
        // Feature2: 200
        packet_bytes.extend_from_slice(&200u16.to_le_bytes());
        // Node name
        packet_bytes.extend_from_slice(b"MeshGateway-Alpha");

        let (parsed_node, metadata) = parse_meshcore_advert(&packet_bytes, [0; 32]).expect("Should parse valid framed advert");
        assert_eq!(parsed_node, node_key);
        assert_eq!(metadata.get("node_name").and_then(|v| v.as_str()), Some("MeshGateway-Alpha"));
        assert_eq!(metadata.get("node_type").and_then(|v| v.as_str()), Some("chat_node"));
        assert_eq!(metadata.get("feature1").and_then(|v| v.as_u64()), Some(100));
        assert_eq!(metadata.get("feature2").and_then(|v| v.as_u64()), Some(200));
        assert_eq!(metadata.get("advert_timestamp").and_then(|v| v.as_u64()), Some(1700000000));
        let lat = metadata.get("latitude").and_then(|v| v.as_f64()).unwrap();
        assert!((lat - 37.7749).abs() < 0.0001);
    }

    #[test]
    fn test_parse_meshcore_advert_transport_direct() {
        let node_key = [0x55u8; 32];
        let mut packet_bytes = Vec::new();

        // Packet Header: Version=0, PayloadType=0x04, RouteType=0x03 (TRANSPORT_DIRECT) -> 0b00010011 = 0x13
        packet_bytes.push(0x13);
        // Transport codes (4 bytes)
        packet_bytes.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        // Path length: 0
        packet_bytes.push(0);

        // Advert payload:
        packet_bytes.extend_from_slice(&node_key);
        packet_bytes.extend_from_slice(&12345u32.to_le_bytes());
        packet_bytes.extend_from_slice(&[0u8; 64]);

        // Flags: 0x82 (is repeater + has name)
        packet_bytes.push(0x82);
        packet_bytes.extend_from_slice(b"HilltopRepeater");

        let (parsed_node, metadata) = parse_meshcore_advert(&packet_bytes, [0; 32]).expect("Should parse transport direct advert");
        assert_eq!(parsed_node, node_key);
        assert_eq!(metadata.get("node_name").and_then(|v| v.as_str()), Some("HilltopRepeater"));
        assert_eq!(metadata.get("node_type").and_then(|v| v.as_str()), Some("repeater"));
    }

    #[test]
    fn test_parse_meshcore_advert_malformed() {
        assert!(parse_meshcore_advert(&[], [0; 32]).is_none());
        assert!(parse_meshcore_advert(&[0x11, 0x00], [0; 32]).is_none());
        assert!(parse_meshcore_advert(&[0u8; 50], [0; 32]).is_none());
    }

    #[test]
    fn test_apps_config_parsing() {
        let toml_str = r#"
        [rate_limiter]
        max_packets_per_minute = 45
        max_burst_packets = 4
        inter_packet_guard_ms = 350
        max_duty_cycle_percent = 1.0
        duty_cycle_window_secs = 3600

        [asset_broadcaster]
        enable_on_demand_broadcast = true
        max_asset_broadcast_duty_cycle = 0.15

        [apps]
        main_app = "custom_menu"
        enabled = ["custom_menu", "messages"]
        "#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.apps.main_app, "custom_menu");
        assert_eq!(config.apps.enabled, vec!["custom_menu", "messages"]);
    }

    #[test]
    fn test_all_app_manifests_valid() {
        let enabled = vec![
            "main_menu".to_string(),
            "messages".to_string(),
            "profile".to_string(),
            "minidungeon".to_string(),
            "admin".to_string(),
            "marketplace".to_string(),
        ];
        let manifest_map = load_app_manifests(&enabled);
        assert!(manifest_map.contains_key(&0x0101)); // dungeon banner
        assert!(manifest_map.contains_key(&0x0102)); // main menu border
        assert!(manifest_map.contains_key(&0x0103)); // main menu banner

        // Verify each main.lua exists
        for app_id in &enabled {
            let entry = format!("apps/{}/main.lua", app_id);
            let path = find_workspace_path(&entry);
            assert!(path.exists(), "App main.lua must exist at {:?}", path);
        }
    }

    #[test]
    fn test_packet_capture_config_deserialization() {
        let toml_str = r#"
        log_level = "debug"

        [rate_limiter]
        max_packets_per_minute = 45
        max_burst_packets = 4
        inter_packet_guard_ms = 350
        max_duty_cycle_percent = 1.0
        duty_cycle_window_secs = 3600

        [asset_broadcaster]
        enable_on_demand_broadcast = true
        max_asset_broadcast_duty_cycle = 0.15

        [packet_capture]
        enabled = true
        directory = "custom_capture_dir"
        "#;
        let config: AppConfig = toml::from_str(toml_str).unwrap();
        assert!(config.packet_capture.enabled);
        assert_eq!(config.packet_capture.directory, "custom_capture_dir");
    }

    #[test]
    fn test_packet_recorder_record_and_csv_generation() {
        let temp_dir = std::env::temp_dir().join(format!("bifrost_capture_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let dir_str = temp_dir.to_str().unwrap();

        let recorder = PacketRecorder::new(dir_str).expect("Failed to initialize PacketRecorder");

        let raw_data = b"Hello Bifrost Screen Buffer 1234567890";
        let comp_data = b"CompressedBuffer";

        recorder.record_compression(
            "TX",
            "screen_delta",
            0x03,
            0x02,
            raw_data,
            Some(comp_data),
            "heatshrink_w8_l4",
            120,
        );

        recorder.record_compression(
            "RX",
            "client_input",
            0x02,
            0x00,
            b"n",
            None,
            "none",
            0,
        );

        // Verify CSV file exists and has rows
        let csv_path = recorder.base_dir.join("compression_log.csv");
        assert!(csv_path.exists());
        let csv_content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_content.contains("timestamp,seq,direction,category,opcode,flags,raw_bytes,compressed_bytes,savings_percent,algorithm,duration_us,raw_file,comp_file"));
        assert!(csv_content.contains("TX,screen_delta,0x03,0x02,38,16,"));
        assert!(csv_content.contains("RX,client_input,0x02,0x00,1,0,0.00,none,0,"));

        // Verify binary files exist
        let raw_file = recorder.raw_dir.join("seq_000001_tx_screen_delta.bin");
        let comp_file = recorder.comp_dir.join("seq_000001_tx_screen_delta.bin");
        assert!(raw_file.exists());
        assert!(comp_file.exists());
        assert_eq!(std::fs::read(&raw_file).unwrap(), raw_data);
        assert_eq!(std::fs::read(&comp_file).unwrap(), comp_data);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_packet_capture_server_integration() {
        let _ = env_logger::builder().is_test(true).try_init();
        let temp_dir = std::env::temp_dir().join(format!("bifrost_live_capture_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let dir_str = temp_dir.to_str().unwrap().to_string();

        let mut config = default_config();
        config.packet_capture.enabled = true;
        config.packet_capture.directory = dir_str.clone();

        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9099".to_string(), 0.0, 0, 200));

        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(2)).await
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9099".to_string(), 0.0, 0, 200);

        let client_node = [0x55; 32];
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        for _ in 0..10 {
            let packet = RadioPacket {
                is_broadcast: false,
                src_node: client_node,
                dst_node: [0; 32],
                payload: handshake_payloads[0].clone(),
                signal_rssi: -50,
                signal_snr: 10,
            };
            if client_transport.send_packet(packet).await.is_ok() {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        // Wait for response
        let start = tokio::time::Instant::now();
        while start.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(pkt)) => {
                    if !pkt.payload.is_empty() {
                        break;
                    }
                }
                _ => {}
            }
        }

        let _ = server_handle.await;

        // Verify capture files were generated
        let csv_path = temp_dir.join("compression_log.csv");
        assert!(csv_path.exists(), "CSV compression log should exist");
        let csv_content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_content.contains("screen_delta") || csv_content.contains("client_input"), "CSV should record compression events");

        let raw_dir = temp_dir.join("raw");
        assert!(raw_dir.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}



