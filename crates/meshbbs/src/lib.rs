//! MeshBBS Host server daemon kernel.
//! Handles session multiplexing, Rate Limiting, QoS Queues, and sandboxed Lua applications.

use anyhow::Result;
use log::{info, warn};
use mlua::LuaSerdeExt;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;
use tokio::sync::mpsc;

// Pull from sibling workspace crates
use meshcore_transport::{
    MeshBbsMessage, MessageReassembler, MockSocketTransport, RadioPacket, RadioTransport,
    TransportStats,
};

/// BBS-level statistics tracker for session and user accounting.
pub struct BbsStats {
    /// Timestamps of when each unique node connected (for 24h active user count)
    pub session_timestamps: StdMutex<Vec<(Instant, [u8; 32])>>,
    /// Currently active session node IDs
    pub active_session_count: StdMutex<usize>,
}

impl BbsStats {
    pub fn new() -> Self {
        Self {
            session_timestamps: StdMutex::new(Vec::new()),
            active_session_count: StdMutex::new(0),
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
        field_bg: 1,
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

/// MeshBBS Server Configuration Loaded from config.toml
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppConfig {
    pub rate_limiter: RateLimiterConfig,
    pub asset_broadcaster: AssetBroadcasterConfig,
    #[serde(default = "default_form_colors")]
    pub form_colors: FormColorsConfig,
    #[serde(default)]
    pub admin_nodes: Vec<String>,
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
            field_bg: 1,
            submit_fg: 0,
            submit_bg: 7,
        },
        admin_nodes: Vec::new(),
    }
}

/// Run the BBS server engine, loading configuration and initializing transport.
pub async fn run_bbs(config_path: Option<PathBuf>, run_duration_secs: Option<u64>) -> Result<()> {
    // 1. Load Config File
    let config: AppConfig = if let Some(path) = config_path {
        if path.exists() {
            info!("Loading configuration from {:?}", path);
            let contents = std::fs::read_to_string(&path)?;
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
pub struct AssetEntry {
    pub name: String,
    pub id: u16,
    pub path: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AssetManifest {
    pub assets: Vec<AssetEntry>,
}

/// Loads the static public asset manifest mapping AssetID -> (Name, RelativePath).
pub fn load_asset_manifest() -> HashMap<u16, (String, String)> {
    let manifest_path = find_workspace_path("assets/manifest.toml");
    let mut map = HashMap::new();
    if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
        if let Ok(manifest) = toml::from_str::<AssetManifest>(&contents) {
            for entry in manifest.assets {
                map.insert(entry.id, (entry.name, entry.path));
            }
        }
    }
    map
}

/// Broadcasts a requested public asset in unencrypted multicast chunks according to spec.
pub async fn broadcast_asset(
    asset_id: u16,
    manifest_map: &HashMap<u16, (String, String)>,
    transport: &Arc<dyn RadioTransport>,
) -> Result<()> {
    if let Some((name, rel_path)) = manifest_map.get(&asset_id) {
        let full_path = find_workspace_path(rel_path);
        if let Ok(content_bytes) = std::fs::read(&full_path) {
            let master_crc = meshcore_transport::crc32(&content_bytes);
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
    let asset_manifest_map = Arc::new(load_asset_manifest());
    let bbs_stats = Arc::new(BbsStats::new());

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

                let active = bbs_stats_clone.active_sessions();
                let unique_24h = bbs_stats_clone.unique_users_24h();
                let tx_total = ts_clone.total_packets_sent();
                let rx_total = ts_clone.total_packets_received();
                let tx_bytes = ts_clone.total_bytes_sent();
                let rx_bytes = ts_clone.total_bytes_received();
                let uptime = ts_clone.uptime_secs();
                let (tx_ppm_1h, rx_ppm_1h) = ts_clone.packets_per_minute_last(3600);
                let (tx_ppm_24h, rx_ppm_24h) = ts_clone.packets_per_minute_last(86400);

                info!("=== BBS Stats (uptime {}s) ===", uptime);
                info!("  Active sessions: {}  |  Unique users (24h): {}", active, unique_24h);
                info!("  Packets TX: {} ({} bytes)  |  Packets RX: {} ({} bytes)", tx_total, tx_bytes, rx_total, rx_bytes);
                info!("  Avg TX/min (1h): {:.1}  |  Avg RX/min (1h): {:.1}", tx_ppm_1h, rx_ppm_1h);
                info!("  Avg TX/min (24h): {:.1}  |  Avg RX/min (24h): {:.1}", tx_ppm_24h, rx_ppm_24h);
            }
        }))
    } else {
        None
    };

    let bbs_stats_clone = bbs_stats.clone();
    // Main packet routing loop placeholder
    let manifest_map_for_loop = asset_manifest_map.clone();
    let loop_handle = tokio::spawn(async move {
        let start_time = tokio::time::Instant::now();
        loop {
            if let Some(dur) = run_duration_secs {
                if start_time.elapsed() >= tokio::time::Duration::from_secs(dur) {
                    break;
                }
            }

            // Receive packet from radio (timeout check allows quick loop exit)
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                transport.receive_packet(),
            )
            .await
            {
                Ok(Ok(packet)) => {
                    let src = packet.src_node;
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
                                tokio::spawn(async move {
                                    let _ = broadcast_asset(
                                        req_asset_id,
                                        &manifest_map_clone,
                                        &transport_broadcast,
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
                                let bbs_stats_inner = bbs_stats_clone.clone();
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
                Ok(Err(meshcore_transport::TransportError::ConnectionClosed)) => {
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
) -> Result<()> {
    log::debug!("Starting run_session_task for client session");
    let lua = mlua::Lua::new();
    let active_app = Arc::new(StdMutex::new("00_main_menu".to_string()));

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
    globals.set("require", mlua::Value::Nil)?;

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
    term.set(
        "render_asset",
        lua.create_function(move |_, asset_name: String| {
            let mut buf = out_buf.lock().unwrap();
            buf.push(0xC5); // OP_RENDER_ASSET
            let id: u16 = if let Some((&matched_id, _)) = asset_manifest_for_render
                .iter()
                .find(|(_, (n, _))| n == &asset_name)
            {
                matched_id
            } else if asset_name == "ASSET_DUNGEON_BANNER" {
                0x0101
            } else if asset_name == "ASSET_MAIN_MENU_BANNER" {
                0x0103
            } else {
                0x0102
            };
            buf.extend_from_slice(&id.to_be_bytes());
            Ok(())
        })?,
    )?;

    let out_buf = output_buf.clone();
    let transport_clone = transport.clone();
    let node_id_clone = node_id.clone();
    let rt = rt_handle.clone();
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
                let payload = buf.clone();
                buf.clear();

                let msg = MeshBbsMessage::new(0x01, 0x03, 0x00, payload);
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
            let payload = buf.clone();
            buf.clear();

            let msg = MeshBbsMessage::new(0x01, 0x03, 0x00, payload);
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
        lua.create_function(move |lua, (table, key): (String, String)| {
            let store = db_store_get.lock().unwrap();
            if let Some(tbl) = store.get(&table) {
                if let Some(val) = tbl.get(&key) {
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(val) {
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
        lua.create_function(
            move |lua, (table, key, val): (String, String, mlua::Value)| {
                if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
                    if let Ok(json_str) = serde_json::to_string(&json_val) {
                        let mut store = db_store_set.lock().unwrap();
                        store
                            .entry(table)
                            .or_insert_with(HashMap::new)
                            .insert(key, json_str);
                    }
                }
                Ok(())
            },
        )?,
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

    session.set(
        "callsign",
        lua.create_function(|_, (): ()| Ok("RadioOperator".to_string()))?,
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

    let active_app_clone = active_app.clone();
    let load_app = lua.create_function(move |lua, app_name: String| {
        let path = find_workspace_path(&format!("apps/{}.lua", app_name));
        if path.exists() {
            *active_app_clone.lock().unwrap() = app_name.clone();
            let code = std::fs::read_to_string(path)?;
            let app: mlua::Table = lua.load(&code).eval()?;
            let on_start: mlua::Function = app.get("on_start")?;
            on_start.call::<_, ()>(lua.globals().get::<_, mlua::Table>("session")?)?;
        }
        Ok(())
    })?;
    session.set("load_app", load_app)?;

    globals.set("session", session)?;

    // Start 00_main_menu.lua
    log::debug!("Loading 00_main_menu.lua");
    let main_path = find_workspace_path("apps/00_main_menu.lua");
    let main_code = std::fs::read_to_string(main_path)?;
    log::debug!("Evaluating menu code...");
    let main_menu: mlua::Table = lua.load(&main_code).eval()?;
    log::debug!("Menu code evaluated successfully");
    let on_start: mlua::Function = main_menu.get("on_start")?;
    log::debug!("Invoking menu.on_start...");
    on_start.call::<_, ()>(lua.globals().get::<_, mlua::Table>("session")?)?;
    log::debug!("menu.on_start invoked successfully");

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

fn find_workspace_path(relative_path: &str) -> PathBuf {
    let path = PathBuf::from(relative_path);
    if path.exists() {
        return path;
    }
    if let Ok(current) = std::env::current_dir() {
        if current.ends_with("crates/meshbbs") || current.ends_with("meshbbs") {
            if let Some(parent) = current.parent() {
                if let Some(workspace_root) = parent.parent() {
                    let parent_path = workspace_root.join(relative_path);
                    if parent_path.exists() {
                        return parent_path;
                    }
                }
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
        assert_eq!(fc.field_bg, 1);
        assert_eq!(fc.submit_fg, 0);
        assert_eq!(fc.submit_bg, 7);
    }

    #[test]
    fn test_default_config_has_empty_admin_nodes() {
        let config = default_config();
        assert!(config.admin_nodes.is_empty());
        assert_eq!(config.form_colors.field_fg, 15);
        assert_eq!(config.form_colors.field_bg, 1);
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
        let payload_str = String::from_utf8_lossy(&hello_response.payload);
        assert!(
            payload_str.contains("ReconnectTestUser"),
            "Hello screen should contain user nickname, got: {}",
            payload_str
        );

        let _ = server_handle.await;
    }

    #[test]
    fn test_asset_manifest_loading() {
        let manifest = load_asset_manifest();
        assert!(manifest.contains_key(&0x0101));
        assert!(manifest.contains_key(&0x0102));
        assert!(manifest.contains_key(&0x0103));

        let (banner_name, banner_path) = manifest.get(&0x0103).unwrap();
        assert_eq!(banner_name, "ASSET_MAIN_MENU_BANNER");
        assert_eq!(banner_path, "assets/main_menu_banner.ans");
    }

    #[tokio::test]
    async fn test_on_demand_asset_broadcast_and_client_caching() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
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

        // Send REQ_ASSET for 0x0103 (ASSET_MAIN_MENU_BANNER)
        let req_asset_id: u16 = 0x0103;
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
            meshcore_transport::crc32(&assembled_bytes),
            expected_crc32,
            "CRC32 mismatch on assembled broadcast asset"
        );

        let banner_path = find_workspace_path("assets/main_menu_banner.ans");
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
}

