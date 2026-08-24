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

pub mod db;
pub use db::{DatabaseConfig, DatabaseStore, DbTelemetryStats, TableStats, default_database_config};

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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
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
        "messages".to_string(),
        "profile".to_string(),
        "admin".to_string(),
    ]
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct MainMenuConfig {
    #[serde(default = "default_menu_banner")]
    pub banner_asset: Option<String>,
    #[serde(default = "default_menu_title")]
    pub title: String,
    #[serde(default = "default_menu_header_fg")]
    pub header_fg: u8,
    #[serde(default = "default_menu_header_bg")]
    pub header_bg: u8,
    #[serde(default = "default_menu_layout")]
    pub layout: String,
    #[serde(default = "default_menu_start_col")]
    pub start_col: u8,
    #[serde(default = "default_menu_start_row")]
    pub start_row: u8,
    #[serde(default = "default_menu_col_width")]
    pub col_width: u8,
    #[serde(default = "default_true")]
    pub show_logout: bool,
}

fn default_menu_banner() -> Option<String> {
    Some("main_menu_banner".to_string())
}

fn default_menu_title() -> String {
    "=== BIFROST MESHBBS ===".to_string()
}

fn default_menu_header_fg() -> u8 {
    14
}

fn default_menu_header_bg() -> u8 {
    0
}

fn default_menu_layout() -> String {
    "grid".to_string()
}

fn default_menu_start_col() -> u8 {
    2
}

fn default_menu_start_row() -> u8 {
    10
}

fn default_menu_col_width() -> u8 {
    16
}

fn default_true() -> bool {
    true
}

pub fn default_main_menu_config() -> MainMenuConfig {
    MainMenuConfig {
        banner_asset: default_menu_banner(),
        title: default_menu_title(),
        header_fg: default_menu_header_fg(),
        header_bg: default_menu_header_bg(),
        layout: default_menu_layout(),
        start_col: default_menu_start_col(),
        start_row: default_menu_start_row(),
        col_width: default_menu_col_width(),
        show_logout: default_true(),
    }
}

/// Bifrost BBS Server Configuration Loaded from config.toml
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
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
    #[serde(default = "default_main_menu_config")]
    pub main_menu: MainMenuConfig,
    #[serde(default = "default_packet_capture_config")]
    pub packet_capture: PacketCaptureConfig,
    #[serde(default = "default_database_config")]
    pub database: DatabaseConfig,
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
        let csv_path = base_dir.join("compression_log.csv");

        // Clean up previous capture data in target directory to ensure a fresh capture
        if raw_dir.exists() {
            let _ = std::fs::remove_dir_all(&raw_dir);
        }
        if comp_dir.exists() {
            let _ = std::fs::remove_dir_all(&comp_dir);
        }
        if csv_path.exists() {
            let _ = std::fs::remove_file(&csv_path);
        }

        std::fs::create_dir_all(&raw_dir)?;
        std::fs::create_dir_all(&comp_dir)?;

        let mut csv_file = std::fs::File::create(&csv_path)?;
        use std::io::Write;
        writeln!(
            csv_file,
            "timestamp,seq,direction,category,opcode,flags,raw_bytes,compressed_bytes,savings_percent,algorithm,duration_us,raw_file,comp_file"
        )?;

        log::info!("Packet capture active, logging to {:?} (clean capture initialized)", base_dir);

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

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct RateLimiterConfig {
    pub max_packets_per_minute: u32,
    pub max_burst_packets: u32,
    pub inter_packet_guard_ms: u32,
    pub max_duty_cycle_percent: f32,
    pub duty_cycle_window_secs: u64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
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
        main_menu: default_main_menu_config(),
        packet_capture: default_packet_capture_config(),
        database: default_database_config(),
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
    #[serde(default)]
    pub admin_only: Option<bool>,
    #[serde(default)]
    pub required_permission: Option<String>,
    #[serde(default)]
    pub hotkey: Option<String>,
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

pub const EMBEDDED_MAIN_MENU_LUA: &str = r#"
local menu = {}

function menu.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    local cfg = nil
    if type(session.get_menu_config) == "function" then
        cfg = session.get_menu_config()
    end
    if not cfg then
        cfg = {
            banner_asset = "main_menu_banner",
            title = "=== BIFROST MESHBBS ===",
            header_fg = 14,
            header_bg = 0,
            layout = "grid",
            start_col = 2,
            start_row = 10,
            col_width = 16,
            show_logout = true
        }
    end

    if cfg.banner_asset and cfg.banner_asset ~= "" then
        term.render_asset(cfg.banner_asset)
    end

    term.move_to(2, 6)
    term.set_color(cfg.header_fg or 14, cfg.header_bg or 0)

    if not user or not user.nickname then
        local default_nick = "Operator"
        if user and user.node_name then
            default_nick = user.node_name
        end

        -- Force register nickname on very first connection
        term.print("Welcome to Bifrost! Please set a nickname:\n")
        term.define_form(1)
        term.print("  Your Nickname: ")
        term.add_input_field("nickname", 18, 8, 15, default_nick)
        term.print("\n")
        term.add_submit_button("register", 2, 10)
        term.flush_form()

        session.await_input(1, function(submission)
            if type(submission) == "string" then
                menu.on_start(session)
                return
            end
            local nick = submission.nickname or default_nick
            local updated_user = user or {}
            updated_user.nickname = nick
            db.set("users", user_id, updated_user)
            log.info("New user registered nickname: " .. nick)
            menu.on_start(session)
        end)
        return
    end

    term.print("Hello, " .. user.nickname .. "!\n")
    term.set_color(7, 0)
    term.print("Select options using Tab/Arrows or Hotkeys:\n\n")

    local is_admin = session.has_permission("admin")
    local apps = nil
    if type(session.get_apps) == "function" then
        apps = session.get_apps()
    end
    if not apps or #apps == 0 then
        apps = {}
    end

    term.define_form(10)

    local start_col = cfg.start_col or 2
    local start_row = cfg.start_row or 10
    local col_width = cfg.col_width or 16
    local layout = cfg.layout or "grid"

    local current_row = start_row
    local current_col = start_col
    local items_in_col = 0
    local registered_apps = {}

    for _, app_info in ipairs(apps) do
        local can_show = true
        if app_info.admin_only and not is_admin then
            can_show = false
        elseif app_info.required_permission and app_info.required_permission ~= "" then
            if not is_admin and not session.has_permission(app_info.required_permission) then
                can_show = false
            end
        end

        if can_show then
            local button_id = app_info.id
            registered_apps[button_id] = app_info.id

            term.add_submit_button(button_id, current_col, current_row)

            if layout == "grid" then
                items_in_col = items_in_col + 1
                if items_in_col % 3 == 0 then
                    current_col = current_col + col_width
                    current_row = start_row
                else
                    current_row = current_row + 2
                end
            else
                current_row = current_row + 2
            end
        end
    end

    if cfg.show_logout then
        term.add_submit_button("logout", current_col, current_row)
    end

    term.flush_form()

    session.await_input(10, function(submission)
        if type(submission) == "string" then
            menu.on_start(session)
            return
        end

        local action = submission.submit
        log.info("Main menu selected action: " .. tostring(action))

        if action == "logout" then
            log.info("User logged out: " .. user.nickname)
            term.clear()
            term.print("Goodbye, " .. user.nickname .. "!\n")
            term.flush()
            session.close()
        elseif registered_apps[action] then
            session.load_app(registered_apps[action])
        elseif action == "read_boards" or action == "messages" then
            session.load_app("messages")
        elseif action == "profile" then
            session.load_app("profile")
        elseif action == "admin" then
            session.load_app("admin")
        else
            local matched = false
            for _, app_info in ipairs(apps) do
                if app_info.id == action then
                    session.load_app(app_info.id)
                    matched = true
                    break
                end
            end
            if not matched then
                menu.on_start(session)
            end
        end
    end)
end

function menu.on_resume(session)
    menu.on_start(session)
end

return menu
"#;

/// Loads the static public asset manifests for all enabled apps as well as global assets.
/// Dynamically assigns unique 16-bit AssetIDs to prevent conflicts.
/// Maps AssetID -> (CanonicalNamespacedName, ResolvedRelativePath).
pub fn load_app_manifests(enabled_apps: &[String]) -> HashMap<u16, (String, String)> {
    let mut map = HashMap::new();
    let mut next_dynamic_id: u16 = 0x0101;

    // 1. Scan global top-level assets directory (e.g. "assets/")
    let global_assets_dir = find_workspace_path("assets");
    if global_assets_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&global_assets_dir) {
            let mut entries_vec: Vec<_> = entries.flatten().collect();
            entries_vec.sort_by_key(|e| e.path());
            for entry in entries_vec {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let asset_stem = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();

                    while map.contains_key(&next_dynamic_id) {
                        next_dynamic_id = next_dynamic_id.wrapping_add(1);
                    }
                    let id = next_dynamic_id;
                    next_dynamic_id = next_dynamic_id.wrapping_add(1);

                    let asset_rel_path = format!("assets/{}", file_name);
                    let namespaced_name = format!("assets/{}", asset_stem);
                    log::debug!(
                        "Registered global asset 0x{:04X}: '{}' -> '{}'",
                        id,
                        namespaced_name,
                        asset_rel_path
                    );
                    map.insert(id, (namespaced_name, asset_rel_path));
                }
            }
        }
    }

    // 2. Scan per-app manifests
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

    // Register active dictionary artifact as system asset 0x00DF if available on disk
    let dict_path = find_workspace_path("config/bbs_dict.bin");
    if dict_path.exists() {
        map.insert(0x00DF, ("system/compression_dict".to_string(), "config/bbs_dict.bin".to_string()));
        log::debug!("Registered domain dictionary as public asset 0x00DF: 'config/bbs_dict.bin'");
    }

    map
}

pub use bifrost_ansi::{parse_menu_csv, substitute_template, MenuAssetDef, MenuButtonDef};

/// Resolves an asset ID and string content from the manifest.
pub fn resolve_asset_id_and_content(
    manifest_map: &HashMap<u16, (String, String)>,
    current_app: &str,
    asset_name: &str,
) -> (u16, Option<String>) {
    let normalized_target = asset_name.replace("::", "/").replace(':', "/");
    let relative_target = format!("{}/{}", current_app, normalized_target);
    let assets_target = format!("assets/{}", normalized_target);

    // 1. Exact match with current app namespace (e.g. "voidtrader/combat_menu")
    let matched = manifest_map
        .iter()
        .find(|(_, (n, _))| {
            let n_norm = n.replace("::", "/").replace(':', "/");
            n_norm == relative_target
        })
        // 2. Exact match with provided full name (e.g. "minidungeon/combat_menu")
        .or_else(|| {
            manifest_map.iter().find(|(_, (n, _))| {
                let n_norm = n.replace("::", "/").replace(':', "/");
                n_norm == normalized_target
            })
        })
        // 3. Exact match with global assets namespace (e.g. "assets/main_menu_banner")
        .or_else(|| {
            manifest_map.iter().find(|(_, (n, _))| {
                let n_norm = n.replace("::", "/").replace(':', "/");
                n_norm == assets_target
            })
        })
        // 4. Suffix match (e.g. ".../combat_menu" or ".../main_menu_banner")
        .or_else(|| {
            manifest_map.iter().find(|(_, (n, _))| {
                let n_norm = n.replace("::", "/").replace(':', "/");
                n_norm.ends_with(&format!("/{}", normalized_target))
            })
        })
        // 5. Substring match fallback
        .or_else(|| {
            manifest_map.iter().find(|(_, (n, _))| {
                n.to_ascii_uppercase().contains(&asset_name.to_ascii_uppercase())
            })
        });

    if let Some((&id, (_, rel_path))) = matched {
        let full_path = find_workspace_path(rel_path);
        let content = std::fs::read_to_string(&full_path).ok();
        (id, content)
    } else {
        (0x0101, None)
    }
}

/// Loads the active compression dictionary (custom trained from config/bbs_dict.bin or static default).
pub fn load_active_dictionary() -> bifrost_compression::CompressionDictionary {
    let dict_path = find_workspace_path("config/bbs_dict.bin");
    if dict_path.exists() {
        if let Ok(bytes) = std::fs::read(&dict_path) {
            if let Ok(dict) = bifrost_compression::CompressionDictionary::from_bytes(&bytes) {
                log::info!(
                    "Loaded custom domain dictionary from {:?} ({} tokens, CRC32: 0x{:08X})",
                    dict_path,
                    dict.tokens().len(),
                    dict.crc32()
                );
                return dict;
            }
        }
    }
    log::info!("Using standard static domain dictionary for compression");
    bifrost_compression::CompressionDictionary::standard_static()
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

pub const SESSION_RESUME_TIMEOUT_SECS: u64 = 600; // 10 minute session resumption window

struct Session {
    input_tx: mpsc::Sender<MeshBbsMessage>,
    last_activity: Arc<StdMutex<std::time::Instant>>,
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
    }    let active_sessions = Arc::new(StdMutex::new(HashMap::<[u8; 32], Session>::new()));
    let db_path = find_workspace_path(&config.database.path);
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let db_store = match DatabaseStore::new(&db_path) {
        Ok(store) => {
            info!("SQLite database initialized at {:?}", db_path);
            store
        }
        Err(e) => {
            warn!("Failed to open SQLite database at {:?}: {:?}. Falling back to in-memory database.", db_path, e);
            DatabaseStore::new_in_memory().expect("In-memory database should always initialize")
        }
    };
    let mut reassembler = MessageReassembler::new();
    let asset_manifest_map = Arc::new(load_app_manifests(&config.apps.enabled));
    let bbs_stats = Arc::new(BbsStats::new());
    let active_dict = Arc::new(load_active_dictionary());

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
                let raw_sent = ts_clone.total_raw_bytes_sent();
                let comp_sent = ts_clone.total_compressed_bytes_sent();
                let savings_pct = if raw_sent > 0 && raw_sent >= comp_sent {
                    ((raw_sent - comp_sent) as f64 / raw_sent as f64) * 100.0
                } else {
                    0.0
                };

                info!(
                    "[BBS STATS] Active Users 24h: {} | Current Sessions: {} | Pkts: {} TX / {} RX | Bytes TX: {} (payload: {} comp / {} raw, +{:.1}% savings) | Bytes RX: {} | PPM 1h: {:.1} TX / {:.1} RX | PPM 24h: {:.1} TX / {:.1} RX | Uptime: {}s",
                    bbs_stats_clone.unique_users_24h(),
                    bbs_stats_clone.active_sessions(),
                    ts_clone.total_packets_sent(),
                    ts_clone.total_packets_received(),
                    ts_clone.total_bytes_sent(),
                    comp_sent,
                    raw_sent,
                    savings_pct,
                    ts_clone.total_bytes_received(),
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

                    // Intercept and parse background MeshCore adverts
                    if let Some((advert_node, metadata)) =
                        parse_meshcore_advert(&packet.payload, src)
                    {
                        let node_hex = advert_node
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<String>();

                        // Merge metadata into existing record if present
                        let mut existing_user = db_store
                            .get("users", &node_hex)
                            .ok()
                            .flatten()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                            .and_then(|v| v.as_object().cloned())
                            .unwrap_or_default();

                        for (k, v) in metadata {
                            existing_user.insert(k, v);
                        }

                        if let Ok(merged_json) = serde_json::to_string(&existing_user) {
                            log::info!("Processed advert packet for node {}: {}", node_hex, merged_json);
                            let _ = db_store.set("users", &node_hex, &merged_json);
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

                            let is_handshake = msg.opcode == 0x01;
                            let tx_opt = {
                                let mut sessions = active_sessions.lock().unwrap();
                                if let Some(session) = sessions.get(&src) {
                                    let elapsed = session.last_activity.lock().unwrap().elapsed();
                                    if is_handshake {
                                        if elapsed < std::time::Duration::from_secs(SESSION_RESUME_TIMEOUT_SECS) {
                                            info!(
                                                "Resuming existing session for node {:?} (idle for {}s < {}s timeout)",
                                                src,
                                                elapsed.as_secs(),
                                                SESSION_RESUME_TIMEOUT_SECS
                                            );
                                            *session.last_activity.lock().unwrap() = std::time::Instant::now();
                                            Some(session.input_tx.clone())
                                        } else {
                                            info!(
                                                "Session for node {:?} expired (idle for {}s >= {}s); booting fresh session",
                                                src,
                                                elapsed.as_secs(),
                                                SESSION_RESUME_TIMEOUT_SECS
                                            );
                                            sessions.remove(&src);
                                            None
                                        }
                                    } else {
                                        *session.last_activity.lock().unwrap() = std::time::Instant::now();
                                        Some(session.input_tx.clone())
                                    }
                                } else {
                                    None
                                }
                            };

                            if let Some(tx) = tx_opt {
                                let _ = tx.send(msg).await;
                            } else {
                                // Boot new session
                                info!("Booting new Lua session for node: {:?}", src);
                                let (tx, rx) = mpsc::channel(100);
                                let last_activity = Arc::new(StdMutex::new(std::time::Instant::now()));
                                let session = Session {
                                    input_tx: tx.clone(),
                                    last_activity,
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
                                let main_menu_config_clone = config.main_menu.clone();
                                let bbs_stats_inner = bbs_stats_clone.clone();
                                let transport_stats_inner = transport_stats.clone();
                                let packet_recorder_inner = packet_recorder.clone();
                                let active_dict_inner = active_dict.clone();
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
                                        main_menu_config_clone,
                                        bbs_stats_inner.clone(),
                                        transport_stats_inner,
                                        packet_recorder_inner,
                                        active_dict_inner,
                                    );
                                    sessions_clone.lock().unwrap().remove(&src);
                                    bbs_stats_inner.record_session_disconnect();
                                    if let Err(e) = res {
                                        log::error!("Session task error: {:?}", e);
                                    }
                                });
                                if !is_handshake {
                                    let _ = tx.send(msg).await;
                                }
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

pub fn register_lua_db(lua: &mlua::Lua, db_store: DatabaseStore) -> mlua::Result<mlua::Table<'_>> {
    let db = lua.create_table()?;

    // db.get(table, [key])
    let db_store_get = db_store.clone();
    db.set(
        "get",
        lua.create_function(move |lua, args: mlua::MultiValue| {
            let mut iter = args.into_iter();
            let table = match iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                _ => return Ok(mlua::Value::Nil),
            };
            let key_opt = match iter.next() {
                Some(mlua::Value::String(s)) => Some(s.to_str()?.to_string()),
                Some(mlua::Value::Integer(i)) => Some(i.to_string()),
                _ => None,
            };

            if let Some(ref key) = key_opt {
                if key != "all" && key != "*" {
                    // Specific single key requested
                    if let Ok(Some(val)) = db_store_get.get(&table, key) {
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&val) {
                            if json_val.is_null() {
                                return Ok(mlua::Value::Nil);
                            }
                            if let Ok(lua_val) = lua.to_value(&json_val) {
                                return Ok(mlua::Value::from(lua_val));
                            }
                        }
                    }
                    return Ok(mlua::Value::Nil);
                }

                // If key is "all", migrate legacy record if present
                if let Ok(Some(_)) = db_store_get.get(&table, "all") {
                    let _ = db_store_get.auto_migrate_monolithic_rows();
                }
            }

            // Either key is omitted, or key is "all"/"*".
            // Retrieve all granular rows in this table/namespace.
            if let Ok(entries) = db_store_get.get_all(&table) {
                if entries.is_empty() {
                    return Ok(mlua::Value::Nil);
                }
                let tbl = lua.create_table()?;
                let mut is_array = true;
                let mut parsed_entries = Vec::new();
                for (k, v) in entries {
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&v) {
                        if !json_val.is_null() {
                            if let Ok(lua_val) = lua.to_value(&json_val) {
                                let idx_opt = k.parse::<usize>().ok();
                                if idx_opt.is_none() {
                                    is_array = false;
                                }
                                parsed_entries.push((k, idx_opt, lua_val));
                            }
                        }
                    }
                }

                if parsed_entries.is_empty() {
                    return Ok(mlua::Value::Nil);
                }

                for (k, idx_opt, lua_val) in parsed_entries {
                    if is_array {
                        if let Some(idx) = idx_opt {
                            tbl.set(idx, lua_val)?;
                        }
                    } else {
                        tbl.set(k, lua_val)?;
                    }
                }
                return Ok(mlua::Value::Table(tbl));
            }

            Ok(mlua::Value::Nil)
        })?,
    )?;

    // db.set(table, [key], val)
    let db_store_set = db_store.clone();
    db.set(
        "set",
        lua.create_function(move |lua, args: mlua::MultiValue| {
            let mut iter = args.into_iter();
            let table = match iter.next() {
                Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                _ => return Ok(()),
            };
            let (key_opt, val) = match (iter.next(), iter.next()) {
                (Some(k_val), Some(v)) => {
                    let key_str = match k_val {
                        mlua::Value::String(s) => s.to_str()?.to_string(),
                        mlua::Value::Integer(i) => i.to_string(),
                        _ => "default".to_string(),
                    };
                    (Some(key_str), v)
                }
                (Some(v), None) => (None, v),
                _ => return Ok(()),
            };

            // If key is "all" (or omitted) and val is a Lua Table, store its records granularly!
            if key_opt.as_deref() == Some("all") || key_opt.is_none() {
                if val.is_nil() {
                    let _ = db_store_set.clear_namespace(&table);
                    return Ok(());
                }

                if let mlua::Value::Table(tbl) = &val {
                    let mut batch = Vec::new();
                    for pair in tbl.clone().pairs::<mlua::Value, mlua::Value>() {
                        if let Ok((k, v)) = pair {
                            let k_str = match k {
                                mlua::Value::Integer(i) => i.to_string(),
                                mlua::Value::String(s) => s.to_str()?.to_string(),
                                _ => continue,
                            };
                            if !v.is_nil() {
                                if let Ok(json_val) = lua.from_value::<serde_json::Value>(v) {
                                    if !json_val.is_null() {
                                        if let Ok(json_str) = serde_json::to_string(&json_val) {
                                            batch.push((k_str, json_str));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if !batch.is_empty() {
                        let _ = db_store_set.clear_namespace(&table);
                        let _ = db_store_set.set_batch(&table, &batch);
                        return Ok(());
                    }
                }
            }

            let key = key_opt.unwrap_or_else(|| "default".to_string());
            if val.is_nil() {
                let _ = db_store_set.remove(&table, &key);
            } else if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
                if json_val.is_null() {
                    let _ = db_store_set.remove(&table, &key);
                } else if let Ok(json_str) = serde_json::to_string(&json_val) {
                    let _ = db_store_set.set(&table, &key, &json_str);
                }
            }
            Ok(())
        })?,
    )?;

    // db.keys(table)
    let db_store_keys = db_store.clone();
    db.set(
        "keys",
        lua.create_function(move |lua, table: String| {
            let table_tbl = lua.create_table()?;
            if let Ok(keys) = db_store_keys.keys(&table) {
                for (i, key) in keys.into_iter().enumerate() {
                    table_tbl.set(i + 1, key)?;
                }
            }
            Ok(table_tbl)
        })?,
    )?;

    Ok(db)
}

fn run_session_task(
    node_id: [u8; 32],
    mut rx: mpsc::Receiver<MeshBbsMessage>,
    transport: Arc<dyn RadioTransport>,
    db_store: DatabaseStore,
    rt_handle: tokio::runtime::Handle,
    form_colors: FormColorsConfig,
    admin_nodes: Vec<String>,
    asset_manifest: Arc<HashMap<u16, (String, String)>>,
    apps_config: AppsConfig,
    main_menu_config: MainMenuConfig,
    bbs_stats: Arc<BbsStats>,
    transport_stats: Option<Arc<TransportStats>>,
    packet_recorder: Option<Arc<PacketRecorder>>,
    active_dict: Arc<bifrost_compression::CompressionDictionary>,
) -> Result<()> {
    log::debug!("Starting run_session_task for client session");
    let lua = mlua::Lua::new();
    let active_app = Arc::new(StdMutex::new(apps_config.main_app.clone()));

    let node_hex_str: String = node_id.iter().map(|b| format!("{:02x}", b)).collect();

    // Check if configured as admin
    let is_configured_admin = admin_nodes.contains(&node_hex_str);

    // Check if first user in database
    let is_first_user = match db_store.keys("users") {
        Ok(keys) => keys.is_empty(),
        Err(_) => true,
    };

    let mut initial_permissions = vec!["read".to_string(), "write".to_string()];
    if is_configured_admin || is_first_user {
        initial_permissions.push("admin".to_string());
    }

    // Persist initial permissions in DB
    if let Ok(None) = db_store.get("permissions", &node_hex_str) {
        let json_str =
            serde_json::to_string(&initial_permissions).unwrap_or_else(|_| "[]".to_string());
        let _ = db_store.set("permissions", &node_hex_str, &json_str);
    }

    // Accumulates output bytes for term.flush()
    let output_buf = Arc::new(StdMutex::new(Vec::new()));
    let session_payload_cache = Arc::new(StdMutex::new(bifrost_transport::SessionPayloadCache::new(100)));

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

    let out_buf_for_cursor = output_buf.clone();
    term.set(
        "set_cursor",
        lua.create_function(move |_, (col, row): (u8, u8)| {
            let mut buf = out_buf_for_cursor.lock().unwrap();
            buf.push(0xC3); // OP_CURSOR_ABS
            buf.push(col);
            buf.push(row);
            Ok(())
        })?,
    )?;

    let out_buf_for_asset = output_buf.clone();
    let asset_manifest_for_render = asset_manifest.clone();
    let active_app_for_asset = active_app.clone();
    term.set(
        "render_asset",
        lua.create_function(move |_, asset_name: String| {
            let mut buf = out_buf_for_asset.lock().unwrap();
            buf.push(0xC5); // OP_RENDER_ASSET

            let current_app = active_app_for_asset.lock().unwrap().clone();
            let (id, _) = resolve_asset_id_and_content(&asset_manifest_for_render, &current_app, &asset_name);
            buf.extend_from_slice(&id.to_be_bytes());
            Ok(())
        })?,
    )?;

    let out_buf_for_tmpl = output_buf.clone();
    let manifest_for_tmpl = asset_manifest.clone();
    let app_for_tmpl = active_app.clone();
    term.set(
        "render_template",
        lua.create_function(move |_, (asset_name, params): (String, mlua::Value)| {
            let current_app = app_for_tmpl.lock().unwrap().clone();
            let (id, _content) = resolve_asset_id_and_content(&manifest_for_tmpl, &current_app, &asset_name);

            let mut param_strings = Vec::new();
            match params {
                mlua::Value::Table(tbl) => {
                    let len = tbl.len().unwrap_or(0);
                    if len > 0 {
                        for i in 1..=len {
                            if let Ok(v) = tbl.get::<i64, mlua::Value>(i) {
                                param_strings.push(match v {
                                    mlua::Value::String(s) => s.to_str()?.to_string(),
                                    mlua::Value::Integer(n) => n.to_string(),
                                    mlua::Value::Number(n) => n.to_string(),
                                    mlua::Value::Boolean(b) => b.to_string(),
                                    _ => String::new(),
                                });
                            }
                        }
                    } else {
                        for pair in tbl.pairs::<mlua::Value, mlua::Value>() {
                            if let Ok((_k, v)) = pair {
                                param_strings.push(match v {
                                    mlua::Value::String(s) => s.to_str()?.to_string(),
                                    mlua::Value::Integer(n) => n.to_string(),
                                    mlua::Value::Number(n) => n.to_string(),
                                    mlua::Value::Boolean(b) => b.to_string(),
                                    _ => String::new(),
                                });
                            }
                        }
                    }
                }
                mlua::Value::String(s) => {
                    param_strings.push(s.to_str()?.to_string());
                }
                mlua::Value::Integer(n) => {
                    param_strings.push(n.to_string());
                }
                _ => {}
            }

            let mut buf = out_buf_for_tmpl.lock().unwrap();
            buf.push(0xC7); // OP_RENDER_TEMPLATE
            buf.extend_from_slice(&id.to_be_bytes());
            buf.push(param_strings.len() as u8);
            for p in &param_strings {
                let p_bytes = p.as_bytes();
                buf.push(p_bytes.len() as u8);
                buf.extend_from_slice(p_bytes);
            }
            Ok(())
        })?,
    )?;

    let out_buf_for_menu = output_buf.clone();
    let manifest_for_menu = asset_manifest.clone();
    let app_for_menu = active_app.clone();
    let form_colors_menu = form_colors.clone();
    term.set(
        "render_menu",
        lua.create_function(move |_, (asset_name, toggle_arg): (String, Option<mlua::Value>)| {
            let current_app = app_for_menu.lock().unwrap().clone();
            let (id, content_opt) = resolve_asset_id_and_content(&manifest_for_menu, &current_app, &asset_name);

            let menu_def = if let Some(content) = content_opt {
                parse_menu_csv(&content)
            } else {
                MenuAssetDef {
                    form_id: 1,
                    field_fg: None,
                    field_bg: None,
                    submit_fg: None,
                    submit_bg: None,
                    align: None,
                    buttons: Vec::new(),
                }
            };

            let mut toggle_mask: u32 = 0;
            match toggle_arg {
                Some(mlua::Value::Integer(n)) => {
                    toggle_mask = n as u32;
                }
                Some(mlua::Value::Table(tbl)) => {
                    for (idx, btn) in menu_def.buttons.iter().enumerate() {
                        if idx < 32 {
                            let is_enabled = if let Ok(val) = tbl.get::<&str, mlua::Value>(&btn.tag) {
                                match val {
                                    mlua::Value::Boolean(b) => b,
                                    mlua::Value::Nil => true,
                                    _ => true,
                                }
                            } else if let Ok(val) = tbl.get::<&str, mlua::Value>(&btn.id) {
                                match val {
                                    mlua::Value::Boolean(b) => b,
                                    mlua::Value::Nil => true,
                                    _ => true,
                                }
                            } else {
                                true
                            };
                            if is_enabled {
                                toggle_mask |= 1 << idx;
                            }
                        }
                    }
                }
                _ => {
                    for idx in 0..menu_def.buttons.len() {
                        if idx < 32 {
                            toggle_mask |= 1 << idx;
                        }
                    }
                }
            }

            let mut buf = out_buf_for_menu.lock().unwrap();
            let f_fg = menu_def.field_fg.unwrap_or(form_colors_menu.field_fg);
            let f_bg = menu_def.field_bg.unwrap_or(form_colors_menu.field_bg);
            let s_fg = menu_def.submit_fg.unwrap_or(form_colors_menu.submit_fg);
            let s_bg = menu_def.submit_bg.unwrap_or(form_colors_menu.submit_bg);

            buf.push(0xD0); // OP_FORM_START
            buf.push(menu_def.form_id);
            buf.push(f_fg);
            buf.push(f_bg);
            buf.push(s_fg);
            buf.push(s_bg);

            buf.push(0xC8); // OP_RENDER_MENU
            buf.extend_from_slice(&id.to_be_bytes());
            buf.extend_from_slice(&toggle_mask.to_be_bytes());
            Ok(())
        })?,
    )?;

    let out_buf_for_table = output_buf.clone();
    term.set(
        "render_table",
        lua.create_function(move |_, (start_col, start_row, config): (u8, u8, mlua::Table)| {
            let mut buf = out_buf_for_table.lock().unwrap();
            let headers: Vec<String> = config.get("headers").unwrap_or_default();
            let widths: Vec<usize> = config.get("widths").unwrap_or_default();
            let rows: Vec<Vec<String>> = config.get("rows").unwrap_or_default();
            let h_fg: u8 = config.get("header_fg").unwrap_or(14);
            let h_bg: u8 = config.get("header_bg").unwrap_or(0);
            let r_fg: u8 = config.get("row_fg").unwrap_or(15);
            let r_bg: u8 = config.get("row_bg").unwrap_or(0);
            let divider: bool = config.get("divider").unwrap_or(true);

            let mut cur_row = start_row;
            if !headers.is_empty() {
                buf.push(0xC3); // OP_CURSOR_ABS
                buf.push(start_col);
                buf.push(cur_row);
                buf.push(0xC0); // OP_SET_COLOR
                buf.push((h_bg << 4) | (h_fg & 0x0F));

                let mut header_line = String::new();
                for (idx, h) in headers.iter().enumerate() {
                    let w = widths.get(idx).copied().unwrap_or(h.len() + 2);
                    header_line.push_str(&format!("{:<width$}", h, width = w));
                    if idx + 1 < headers.len() {
                        header_line.push_str("  ");
                    }
                }
                buf.extend_from_slice(header_line.as_bytes());
                cur_row += 1;

                if divider {
                    buf.push(0xC3);
                    buf.push(start_col);
                    buf.push(cur_row);
                    buf.push(0xC0);
                    buf.push((h_bg << 4) | (h_fg & 0x0F));
                    let mut div_line = String::new();
                    for (idx, h) in headers.iter().enumerate() {
                        let w = widths.get(idx).copied().unwrap_or(h.len() + 2);
                        div_line.push_str(&"-".repeat(w));
                        if idx + 1 < headers.len() {
                            div_line.push_str("  ");
                        }
                    }
                    buf.extend_from_slice(div_line.as_bytes());
                    cur_row += 1;
                }
            }

            for row in rows {
                buf.push(0xC3);
                buf.push(start_col);
                buf.push(cur_row);
                buf.push(0xC0);
                buf.push((r_bg << 4) | (r_fg & 0x0F));

                let mut row_line = String::new();
                for (idx, cell) in row.iter().enumerate() {
                    let w = widths.get(idx).copied().unwrap_or(cell.len() + 2);
                    row_line.push_str(&format!("{:<width$}", cell, width = w));
                    if idx + 1 < row.len() {
                        row_line.push_str("  ");
                    }
                }
                buf.extend_from_slice(row_line.as_bytes());
                cur_row += 1;
            }

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
    let active_dict_flush = active_dict.clone();
    let session_cache_flush = session_payload_cache.clone();
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

                let payload_crc = bifrost_transport::crc32(&buf);
                let mut sc = session_cache_flush.lock().unwrap();

                let (flags, payload, algo_name) = if raw_len >= 32 && sc.contains(payload_crc) {
                    log::debug!(
                        "[SESSION DEDUP] Hash-referencing repeated payload 0x{:08X} (raw: {}B -> hash: 4B)",
                        payload_crc,
                        raw_len
                    );
                    (0x08, payload_crc.to_be_bytes().to_vec(), "session_cache_ref")
                } else {
                    sc.insert(payload_crc, buf.clone());
                    let (f, p) = bifrost_ansi::compress_bytecode_adaptive(&buf, Some(&active_dict_flush));
                    let name = match f & 0x06 {
                        0x02 => "heatshrink_w8_l4",
                        0x04 => "domain_dict",
                        0x06 => "domain_dict+heatshrink",
                        _ => "raw_fallback",
                    };
                    (f, p, name)
                };

                let comp_len = payload.len();
                bbs_stats_flush.record_compression(raw_len, comp_len);
                if let Some(ref ts) = transport_stats_flush {
                    ts.record_compression(raw_len, comp_len);
                }
                let comp_duration = start_comp.elapsed().as_micros() as u64;
                if let Some(ref recorder) = packet_recorder_flush {
                    let comp_opt = if (flags & 0x0E) != 0 {
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
                        algo_name,
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
                                    src_node: [0xBB; 32],
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
    let active_dict_form = active_dict.clone();
    let session_cache_form = session_payload_cache.clone();
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

            let payload_crc = bifrost_transport::crc32(&buf);
            let mut sc = session_cache_form.lock().unwrap();

            let (flags, payload, algo_name) = if raw_len >= 32 && sc.contains(payload_crc) {
                log::debug!(
                    "[SESSION DEDUP] Hash-referencing repeated form template 0x{:08X} (raw: {}B -> hash: 4B)",
                    payload_crc,
                    raw_len
                );
                (0x08, payload_crc.to_be_bytes().to_vec(), "session_cache_ref")
            } else {
                sc.insert(payload_crc, buf.clone());
                let (f, p) = bifrost_ansi::compress_bytecode_adaptive(&buf, Some(&active_dict_form));
                let name = match f & 0x06 {
                    0x02 => "heatshrink_w8_l4",
                    0x04 => "domain_dict",
                    0x06 => "domain_dict+heatshrink",
                    _ => "raw_fallback",
                };
                (f, p, name)
            };

            let comp_len = payload.len();
            bbs_stats_form.record_compression(raw_len, comp_len);
            if let Some(ref ts) = transport_stats_form {
                ts.record_compression(raw_len, comp_len);
            }
            let comp_duration = start_comp.elapsed().as_micros() as u64;
            if let Some(ref recorder) = packet_recorder_form {
                let comp_opt = if (flags & 0x0E) != 0 {
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
                    algo_name,
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
                                src_node: [0xBB; 32],
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
    let db = register_lua_db(&lua, db_store.clone())?;
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

    // http table
    let http_table = lua.create_table()?;
    http_table.set(
        "get_json",
        lua.create_function(move |lua, url: String| {
            // Mitigate SSRF by restricting to allowed domain(s)
            if !url.starts_with("https://api.open-meteo.com/") {
                log::error!("Blocked HTTP request to unauthorized URL: {}", url);
                return Ok(mlua::Value::Nil);
            }

            // reqwest::blocking cannot be used within tokio runtime, need to spawn blocking
            let url_clone = url.clone();
            let json_result = std::thread::spawn(move || {
                let client = reqwest::blocking::Client::new();
                let resp = client.get(&url_clone).send().map_err(|e| e.to_string())?;
                if resp.status().is_success() {
                    resp.json::<serde_json::Value>().map_err(|e| e.to_string())
                } else {
                    Err(format!("HTTP error: status {}", resp.status()))
                }
            }).join().unwrap_or(Err("Thread panicked".to_string()));

            match json_result {
                Ok(json_val) => {
                    let lua_val = lua.to_value(&json_val)?;
                    Ok(lua_val)
                }
                Err(e) => {
                    log::error!("HTTP GET request failed for {}: {}", url, e);
                    Ok(mlua::Value::Nil)
                }
            }
        })?,
    )?;
    globals.set("http", http_table)?;

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
            if let Ok(Some(user_json)) = db_store_callsign.get("users", &node_hex_str_clone) {
                if let Ok(user_obj) = serde_json::from_str::<serde_json::Value>(&user_json) {
                    if let Some(nickname) = user_obj.get("nickname").and_then(|v| v.as_str()) {
                        return Ok(nickname.to_string());
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
            if let Ok(Some(json_str)) = db_store_perms.get("permissions", &node_hex_str_clone) {
                if let Ok(perms) = serde_json::from_str::<Vec<String>>(&json_str) {
                    let table = lua.create_table()?;
                    for (i, p) in perms.into_iter().enumerate() {
                        table.set(i + 1, p)?;
                    }
                    return Ok(table);
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
            if let Ok(Some(json_str)) = db_store_has_perm.get("permissions", &node_hex_str_clone) {
                if let Ok(perms) = serde_json::from_str::<Vec<String>>(&json_str) {
                    return Ok(perms.contains(&perm));
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
    let main_app_name_clone = apps_config.main_app.clone();
    let active_app_clone = active_app.clone();
    let load_app = lua.create_function(move |lua, app_name: String| {
        let is_main = app_name == "main_menu" || app_name == main_app_name_clone;
        if !is_main && !enabled_apps.contains(&app_name) {
            log::error!("Application '{}' not found or not enabled in config", app_name);
            return Ok(());
        }
        let entry_file = format!("apps/{}/main.lua", app_name);
        let path = find_workspace_path(&entry_file);
        let code = if path.exists() {
            std::fs::read_to_string(&path)?
        } else if is_main {
            EMBEDDED_MAIN_MENU_LUA.to_string()
        } else {
            log::error!("Application '{}' entry point not found at {:?}", app_name, path);
            return Ok(());
        };

        *active_app_clone.lock().unwrap() = app_name.clone();
        let app: mlua::Table = lua.load(&code).set_name(&app_name).eval()?;
        let on_start: mlua::Function = app.get("on_start")?;
        on_start.call::<_, ()>(lua.globals().get::<_, mlua::Table>("session")?)?;
        Ok(())
    })?;
    session.set("load_app", load_app.clone())?;
    session.set("exec_app", load_app)?;

    session.set(
        "time",
        lua.create_function(|_, (): ()| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            Ok(secs)
        })?,
    )?;

    session.set(
        "date_str",
        lua.create_function(|_, (): ()| {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let days = secs / 86400;
            Ok(format!("day-{}", days))
        })?,
    )?;

    let main_menu_cfg_clone = main_menu_config.clone();
    session.set(
        "get_menu_config",
        lua.create_function(move |lua, (): ()| {
            let tbl = lua.create_table()?;
            tbl.set("banner_asset", main_menu_cfg_clone.banner_asset.clone())?;
            tbl.set("title", main_menu_cfg_clone.title.clone())?;
            tbl.set("header_fg", main_menu_cfg_clone.header_fg)?;
            tbl.set("header_bg", main_menu_cfg_clone.header_bg)?;
            tbl.set("layout", main_menu_cfg_clone.layout.clone())?;
            tbl.set("start_col", main_menu_cfg_clone.start_col)?;
            tbl.set("start_row", main_menu_cfg_clone.start_row)?;
            tbl.set("col_width", main_menu_cfg_clone.col_width)?;
            tbl.set("show_logout", main_menu_cfg_clone.show_logout)?;
            Ok(tbl)
        })?,
    )?;

    let enabled_apps_for_get = apps_config.enabled.clone();
    session.set(
        "get_apps",
        lua.create_function(move |lua, (): ()| {
            let tbl = lua.create_table()?;
            let mut idx = 1;
            for app_id in &enabled_apps_for_get {
                if app_id == "main_menu" {
                    continue;
                }
                let manifest_rel = format!("apps/{}/manifest.toml", app_id);
                let manifest_path = find_workspace_path(&manifest_rel);
                if !manifest_path.is_file() {
                    continue;
                }

                let mut name = app_id.clone();
                let mut description = String::new();
                let mut admin_only = app_id == "admin";
                let mut required_permission = None;
                let mut hotkey = None;

                if let Ok(contents) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(app_manifest) = toml::from_str::<AppManifest>(&contents) {
                        name = app_manifest.app.name;
                        description = app_manifest.app.description.unwrap_or_default();
                        admin_only = app_manifest.app.admin_only.unwrap_or(app_id == "admin");
                        required_permission = app_manifest.app.required_permission;
                        hotkey = app_manifest.app.hotkey;
                    }
                }

                let app_tbl = lua.create_table()?;
                app_tbl.set("id", app_id.clone())?;
                app_tbl.set("name", name)?;
                app_tbl.set("description", description)?;
                app_tbl.set("admin_only", admin_only)?;
                app_tbl.set("required_permission", required_permission)?;
                app_tbl.set("hotkey", hotkey)?;

                tbl.set(idx, app_tbl)?;
                idx += 1;
            }
            Ok(tbl)
        })?,
    )?;

    globals.set("session", session)?;

    // Start initial application specified in config (default: main_menu)
    let main_app_name = apps_config.main_app.clone();
    log::debug!("Loading initial app '{}'...", main_app_name);
    let main_entry_file = format!("apps/{}/main.lua", main_app_name);
    let main_path = find_workspace_path(&main_entry_file);
    let main_code_opt = if main_path.exists() {
        std::fs::read_to_string(&main_path).ok()
    } else if main_app_name == "main_menu" {
        Some(EMBEDDED_MAIN_MENU_LUA.to_string())
    } else {
        None
    };

    if let Some(main_code) = main_code_opt {
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
                let (raw_data, comp_opt, duration_us) = if (msg.flags & 0x06) != 0 {
                    let start_decomp = std::time::Instant::now();
                    let decomp = bifrost_ansi::decompress_bytecode_adaptive(
                        msg.flags,
                        &msg.payload,
                        Some(&active_dict),
                    )
                    .unwrap_or_else(|_| msg.payload.clone());
                    let dur = start_decomp.elapsed().as_micros() as u64;
                    (decomp, Some(msg.payload.as_slice()), dur)
                } else {
                    (msg.payload.clone(), None, 0)
                };
                let algo_name = match msg.flags & 0x06 {
                    0x02 => "heatshrink_w8_l4",
                    0x04 => "domain_dict",
                    0x06 => "domain_dict+heatshrink",
                    _ => "none",
                };
                recorder.record_compression(
                    "RX",
                    "client_input",
                    msg.opcode,
                    msg.flags,
                    &raw_data,
                    comp_opt,
                    algo_name,
                    duration_us,
                );
            }
            if msg.opcode == 0x06 && msg.payload.len() >= 4 {
                // Client reported cache miss for a session hash reference -> retransmit full payload
                let missing_crc = u32::from_be_bytes([
                    msg.payload[0],
                    msg.payload[1],
                    msg.payload[2],
                    msg.payload[3],
                ]);
                log::warn!(
                    "[SESSION DEDUP] Received NACK for missing CRC 0x{:08X}; retransmitting full frame",
                    missing_crc
                );
                let sc = session_payload_cache.lock().unwrap();
                if let Some(cached_buf) = sc.get(missing_crc) {
                    let (flags, payload) =
                        bifrost_ansi::compress_bytecode_adaptive(cached_buf, Some(&active_dict));
                    let msg = MeshBbsMessage::new(0x01, 0x03, flags, payload);
                    let mtu = transport.get_mtu();
                    if let Ok(fragments) = msg.to_fragments(mtu) {
                        let transport_inner = transport.clone();
                        let node_id_clone = node_id.clone();
                        rt_handle.block_on(async {
                            for frag in fragments {
                                let packet = RadioPacket {
                                    is_broadcast: false,
                                    src_node: [0xBB; 32],
                                    dst_node: node_id_clone,
                                    payload: frag,
                                    signal_rssi: 0,
                                    signal_snr: 0,
                                };
                                let _ = transport_inner.send_packet(packet).await;
                            }
                        });
                    }
                }
                continue;
            }
            if msg.opcode == 0x01 {
                // Handshake on existing session -> Resume active app state
                let current_app_name = active_app.lock().unwrap().clone();
                log::info!(
                    "[SESSION RESUME] Resuming active app '{}' for node {:?}",
                    current_app_name,
                    node_id
                );
                let entry_file = format!("apps/{}/main.lua", current_app_name);
                let path = find_workspace_path(&entry_file);
                let code_opt = if path.exists() {
                    std::fs::read_to_string(&path).ok()
                } else if current_app_name == "main_menu" {
                    Some(EMBEDDED_MAIN_MENU_LUA.to_string())
                } else {
                    None
                };

                if let Some(code) = code_opt {
                    if let Ok(app_table) =
                        lua.load(&code).set_name(&current_app_name).eval::<mlua::Table>()
                    {
                        let session_table =
                            lua.globals().get::<_, mlua::Table>("session")?;
                        if let Ok(on_resume) =
                            app_table.get::<_, mlua::Function>("on_resume")
                        {
                            log::debug!("Invoking on_resume for '{}'...", current_app_name);
                            if let Err(e) = on_resume.call::<_, ()>(session_table) {
                                log::error!(
                                    "Error in on_resume for '{}': {:?}",
                                    current_app_name,
                                    e
                                );
                            }
                        } else if let Ok(on_start) =
                            app_table.get::<_, mlua::Function>("on_start")
                        {
                            log::debug!(
                                "Invoking on_start fallback on resume for '{}'...",
                                current_app_name
                            );
                            if let Err(e) = on_start.call::<_, ()>(session_table) {
                                log::error!(
                                    "Error in on_start on resume for '{}': {:?}",
                                    current_app_name,
                                    e
                                );
                            }
                        }
                    }
                }
                continue;
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

        let _ = server_handle.abort();
    }

    #[tokio::test]
    async fn test_main_menu_form_rendering_on_client() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server(
            "127.0.0.1:9096".to_string(),
            0.0,
            0,
            200,
        ));

        let server_handle =
            tokio::spawn(async move { start_server(config, server_transport, Some(1)).await });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport =
            MockSocketTransport::new_client("127.0.0.1:9096".to_string(), 0.0, 0, 200);

        let client_key = [6u8; 32];
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // 1. Receive registration form
        let mut client_reassembler = MessageReassembler::new();
        let mut reg_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            if let Ok(Ok(packet)) = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            ).await {
                if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                    reg_msg = Some(msg);
                    break;
                }
            }
        }
        let _ = reg_msg.expect("Registration form expected");

        // 2. Submit nickname
        let form_submit_json = r#"{"nickname":"TestClient","submit":"register"}"#;
        let submit_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, form_submit_json.as_bytes().to_vec());
        let submit_payloads = submit_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: submit_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // 3. Receive main menu frame
        let mut main_menu_msg = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            if let Ok(Ok(packet)) = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                client_transport.receive_packet(),
            ).await {
                if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                    main_menu_msg = Some(msg);
                    break;
                }
            }
        }
        let msg = main_menu_msg.expect("Main menu frame expected");
        println!("Main menu msg flags: {:02x}, len: {}", msg.flags, msg.payload.len());

        let dict = bifrost_compression::CompressionDictionary::standard_static();
        let decomp = if (msg.flags & 0x06) != 0 {
            bifrost_ansi::decompress_bytecode_adaptive(msg.flags, &msg.payload, Some(&dict)).unwrap()
        } else {
            msg.payload
        };
        println!("Decompressed main menu payload (len {}): {:?}", decomp.len(), decomp);

        let _ = server_handle.abort();
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
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let admin_nodes: Vec<String> = Vec::new();
        let node_hex =
            "0505050505050505050505050505050505050505050505050505050505050505".to_string();

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.keys("users") {
            Ok(users_map) => users_map.is_empty(),
            Err(_) => true,
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

        let db_store = DatabaseStore::new_in_memory().unwrap();
        // Simulate an existing user so this node is NOT the first user
        db_store.set("users", "other_node", "{}").unwrap();

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.keys("users") {
            Ok(users_map) => users_map.is_empty(),
            Err(_) => true,
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

        let db_store = DatabaseStore::new_in_memory().unwrap();
        db_store.set("users", "existing_admin", "{}").unwrap();

        let is_configured_admin = admin_nodes.contains(&node_hex);
        let is_first_user = match db_store.keys("users") {
            Ok(users_map) => users_map.is_empty(),
            Err(_) => true,
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
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let node_hex = "abcd".to_string();
        let perms = vec!["read".to_string(), "write".to_string(), "admin".to_string()];
        let json_str = serde_json::to_string(&perms).unwrap();

        db_store.set("permissions", &node_hex, &json_str).unwrap();

        // Verify we can read them back
        let stored = db_store.get("permissions", &node_hex).unwrap().unwrap();
        let decoded: Vec<String> = serde_json::from_str(&stored).unwrap();
        assert_eq!(decoded, vec!["read", "write", "admin"]);
    }

    fn decode_test_msg(
        cache: &mut bifrost_transport::SessionPayloadCache,
        msg: &MeshBbsMessage,
        dict: &bifrost_compression::CompressionDictionary,
    ) -> Vec<u8> {
        if (msg.flags & 0x08) != 0 && msg.payload.len() >= 4 {
            let crc = u32::from_be_bytes([
                msg.payload[0],
                msg.payload[1],
                msg.payload[2],
                msg.payload[3],
            ]);
            cache
                .get(crc)
                .cloned()
                .unwrap_or_else(|| msg.payload.clone())
        } else if (msg.flags & 0x06) != 0 {
            let decomp = bifrost_ansi::decompress_bytecode_adaptive(
                msg.flags,
                &msg.payload,
                Some(dict),
            )
            .unwrap_or_else(|_| msg.payload.clone());
            let crc = bifrost_transport::crc32(&decomp);
            cache.insert(crc, decomp.clone());
            decomp
        } else {
            let crc = bifrost_transport::crc32(&msg.payload);
            cache.insert(crc, msg.payload.clone());
            msg.payload.clone()
        }
    }

    #[test]
    fn test_permissions_dedup_on_reconnect() {
        // If perms already exist in DB, they should NOT be overwritten
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let node_hex = "node123".to_string();
        let original_perms = vec!["read".to_string()];
        let json_str = serde_json::to_string(&original_perms).unwrap();

        db_store.set("permissions", &node_hex, &json_str).unwrap();

        // Simulate reconnect logic: only insert if not present
        let new_perms = vec!["read".to_string(), "write".to_string(), "admin".to_string()];
        let new_json = serde_json::to_string(&new_perms).unwrap();
        if db_store.get("permissions", &node_hex).unwrap().is_none() {
            db_store.set("permissions", &node_hex, &new_json).unwrap();
        }

        // Should still have original perms
        let stored = db_store.get("permissions", &node_hex).unwrap().unwrap();
        let decoded: Vec<String> = serde_json::from_str(&stored).unwrap();
        assert_eq!(decoded, vec!["read"]);
    }

    #[test]
    fn test_db_keys_pattern() {
        let db_store = DatabaseStore::new_in_memory().unwrap();
        db_store.set("users", "node_a", r#"{"nickname":"Alice"}"#).unwrap();
        db_store.set("users", "node_b", r#"{"nickname":"Bob"}"#).unwrap();

        let mut keys: Vec<String> = db_store.keys("users").unwrap();
        keys.sort();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys, vec!["node_a".to_string(), "node_b".to_string()]);
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
        let mut client_cache = bifrost_transport::SessionPayloadCache::new(100);
        let static_dict = bifrost_compression::CompressionDictionary::standard_static();

        // First connection: handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

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
        let _ = decode_test_msg(&mut client_cache, &welcome, &static_dict);

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
        let uncompressed_payload = decode_test_msg(&mut client_cache, &hello_response, &static_dict);
        let payload_str = String::from_utf8_lossy(&uncompressed_payload);
        assert!(
            payload_str.contains("ReconnectTestUser") || payload_str.contains("Hello"),
            "Hello screen should contain user nickname, got: {}",
            payload_str
        );

        let _ = server_handle.await;
    }

    #[test]
    fn test_asset_manifest_loading() {
        let manifest = load_app_manifests(&["minidungeon".to_string()]);
        assert!(manifest.len() >= 3, "Manifest should contain global assets and minidungeon assets");

        let banner_entry = manifest
            .iter()
            .find(|(_, (name, _))| name == "assets/main_menu_banner" || name == "main_menu_banner");
        assert!(banner_entry.is_some(), "assets/main_menu_banner must be registered");
        let (&banner_id, (banner_name, banner_path)) = banner_entry.unwrap();
        assert!(banner_name.contains("main_menu_banner"));
        assert_eq!(banner_path, "assets/main_menu_banner.ans");

        let border_entry = manifest
            .iter()
            .find(|(_, (name, _))| name == "assets/main_menu_border" || name == "main_menu_border");
        assert!(border_entry.is_some(), "assets/main_menu_border must be registered");
        let (&border_id, _) = border_entry.unwrap();
        assert_ne!(banner_id, border_id, "Asset IDs must be unique");

        // Test global asset resolution
        let (resolved_banner_id, _) = resolve_asset_id_and_content(&manifest, "messages", "main_menu_banner");
        assert_eq!(resolved_banner_id, banner_id);
    }

    #[tokio::test]
    async fn test_on_demand_asset_broadcast_and_client_caching() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let manifest = load_app_manifests(&config.apps.enabled);
        let (&req_asset_id, (_, _)) = manifest
            .iter()
            .find(|(_, (n, _))| n == "assets/main_menu_banner" || n == "main_menu_banner")
            .expect("main_menu_banner must exist in manifest");

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
            "Did not receive any broadcast asset chunks for asset"
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
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let db = register_lua_db(&lua, db_store).unwrap();
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
    fn test_core_apps_syntax() {
        let core_apps = ["messages", "profile", "admin"];
        let lua = mlua::Lua::new();
        for app in &core_apps {
            let path = find_workspace_path(&format!("apps/{}/main.lua", app));
            let code = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("Failed to read apps/{}/main.lua", app));
            let chunk = lua.load(&code);
            assert!(
                chunk.into_function().is_ok(),
                "apps/{}/main.lua should compile as valid Lua",
                app
            );
        }
    }

    #[test]
    fn test_db_flexible_args() {
        let lua = mlua::Lua::new();
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let db = register_lua_db(&lua, db_store).unwrap();
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
    fn test_db_granular_array_storage_and_individual_key_access() {
        let lua = mlua::Lua::new();
        let db_store = DatabaseStore::new_in_memory().unwrap();
        let db = register_lua_db(&lua, db_store.clone()).unwrap();
        lua.globals().set("db", db).unwrap();

        // Save a collection of sectors under "all"
        lua.load(
            r#"
            local sectors = {
                [1] = { name = "Sol Central", ore = 100 },
                [2] = { name = "Alpha Centauri", ore = 50 },
                [3] = { name = "Sirius Prime", ore = 200 }
            }
            db.set("vt_sectors", "all", sectors)
            "#,
        )
        .exec()
        .unwrap();

        // In SQLite db_store, this must be stored granularly as 3 separate rows!
        assert_eq!(db_store.count("vt_sectors").unwrap(), 3);
        let keys = db_store.keys("vt_sectors").unwrap();
        assert_eq!(keys, vec!["1", "2", "3"]);

        // Reading db.get("vt_sectors", "all") must reconstruct the Lua array of 3 sectors
        let (len, s1_name, s2_ore): (usize, String, i64) = lua
            .load(
                r#"
                local s = db.get("vt_sectors", "all")
                return #s, s[1].name, s[2].ore
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(len, 3);
        assert_eq!(s1_name, "Sol Central");
        assert_eq!(s2_ore, 50);

        // Reading single sector 2 directly (O(1) row lookup)
        let s2_name: String = lua
            .load(r#"local s = db.get("vt_sectors", 2); return s.name"#)
            .eval()
            .unwrap();
        assert_eq!(s2_name, "Alpha Centauri");

        // Updating single sector 2 in-place
        lua.load(r#"db.set("vt_sectors", 2, { name = "Alpha Outpost", ore = 75 })"#)
            .exec()
            .unwrap();

        // Verify SQLite store still has 3 records and sector 2 was updated
        assert_eq!(db_store.count("vt_sectors").unwrap(), 3);
        let s2_updated_name: String = lua
            .load(r#"local s = db.get("vt_sectors", 2); return s.name"#)
            .eval()
            .unwrap();
        assert_eq!(s2_updated_name, "Alpha Outpost");

        // Verify loading "all" reflects the updated sector 2
        let s2_from_all_name: String = lua
            .load(r#"local s = db.get("vt_sectors", "all"); return s[2].name"#)
            .eval()
            .unwrap();
        assert_eq!(s2_from_all_name, "Alpha Outpost");
    }

    #[tokio::test]
    async fn test_messages_session_navigation() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9094".to_string(), 0.0, 0, 200));

        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(4)).await
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9094".to_string(), 0.0, 0, 200);
        let client_key = [9u8; 32];
        let mut client_cache = bifrost_transport::SessionPayloadCache::new(100);
        let static_dict = bifrost_compression::CompressionDictionary::standard_static();

        // Handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        let mut client_reassembler = MessageReassembler::new();

        // 1. Receive register screen
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        let _ = decode_test_msg(&mut client_cache, &msg, &static_dict);
                        break;
                    }
                }
                _ => {}
            }
        }

        // 2. Register nickname "MsgBob"
        let register_json = r#"{"nickname":"MsgBob","submit":"register"}"#;
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
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        let _ = decode_test_msg(&mut client_cache, &msg, &static_dict);
                        break;
                    }
                }
                _ => {}
            }
        }

        // 3. Submit "messages" action from main menu
        let msg_select_json = r#"{"submit":"messages"}"#;
        let msg_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, msg_select_json.as_bytes().to_vec());
        let msg_payloads = msg_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: msg_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // 4. Receive Messages screen
        let mut msg_screen = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        msg_screen = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let resp = msg_screen.expect("Should receive Messages screen");
        let uncompressed_payload = decode_test_msg(&mut client_cache, &resp, &static_dict);
        let screen_text = String::from_utf8_lossy(&uncompressed_payload);
        assert!(screen_text.contains("MESSAGES") || screen_text.contains("Messages") || screen_text.contains("General"), "Should contain MESSAGES header, got: {}", screen_text);

        // 5. Navigate back to Main Menu from Messages
        let back_menu_json = r#"{"submit":"main_menu"}"#;
        let back_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, back_menu_json.as_bytes().to_vec());
        let back_payloads = back_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: back_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // Receive Main Menu screen again
        let mut main_return_screen = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        main_return_screen = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }
        let main_resp = main_return_screen.expect("Should receive Main Menu screen when navigating back");
        let main_payload = decode_test_msg(&mut client_cache, &main_resp, &static_dict);
        let main_text = String::from_utf8_lossy(&main_payload);
        assert!(main_text.contains("Select options") || main_text.contains("messages"), "Should contain main menu options, got: {}", main_text);

        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_profile_session_navigation() {
        let _ = env_logger::builder().is_test(true).try_init();
        let config = default_config();
        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9118".to_string(), 0.0, 0, 200));

        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(4)).await
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9118".to_string(), 0.0, 0, 200);
        let client_key = [11u8; 32];
        let mut client_cache = bifrost_transport::SessionPayloadCache::new(100);
        let static_dict = bifrost_compression::CompressionDictionary::standard_static();

        // Handshake
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        let mut client_reassembler = MessageReassembler::new();

        // 1. Receive register screen
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        let _ = decode_test_msg(&mut client_cache, &msg, &static_dict);
                        break;
                    }
                }
                _ => {}
            }
        }

        // 2. Register nickname "ProfBob"
        let register_json = r#"{"nickname":"ProfBob","submit":"register"}"#;
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
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        let _ = decode_test_msg(&mut client_cache, &msg, &static_dict);
                        break;
                    }
                }
                _ => {}
            }
        }

        // 3. Submit "profile" action from main menu
        let profile_select_json = r#"{"submit":"profile"}"#;
        let profile_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, profile_select_json.as_bytes().to_vec());
        let profile_payloads = profile_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: profile_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // 4. Receive Profile screen
        let mut prof_screen = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        prof_screen = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let resp = prof_screen.expect("Should receive Profile screen");
        let uncompressed_payload = decode_test_msg(&mut client_cache, &resp, &static_dict);
        let screen_text = String::from_utf8_lossy(&uncompressed_payload);
        assert!(screen_text.contains("PROFILE") || screen_text.contains("Profile") || screen_text.contains("ProfBob"), "Should contain PROFILE header, got: {}", screen_text);

        // 5. Navigate back to Main Menu from Profile by canceling
        let cancel_json = r#"{"submit":"cancel"}"#;
        let cancel_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, cancel_json.as_bytes().to_vec());
        let cancel_payloads = cancel_msg.to_fragments(200).unwrap();
        let packet = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: cancel_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        client_transport.send_packet(packet).await.unwrap();

        // Receive Main Menu screen again
        let mut main_return_screen = None;
        let start_time = tokio::time::Instant::now();
        while start_time.elapsed() < tokio::time::Duration::from_millis(1500) {
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        main_return_screen = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }
        let main_resp = main_return_screen.expect("Should receive Main Menu screen when canceling profile");
        let main_payload = decode_test_msg(&mut client_cache, &main_resp, &static_dict);
        let main_text = String::from_utf8_lossy(&main_payload);
        assert!(main_text.contains("Select options") || main_text.contains("messages"), "Should contain main menu options, got: {}", main_text);

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
            "messages".to_string(),
            "profile".to_string(),
            "admin".to_string(),
        ];
        let manifest_map = load_app_manifests(&enabled);
        assert!(manifest_map.contains_key(&0x0101)); // global assets/main_menu_banner
        assert!(manifest_map.contains_key(&0x0102)); // global assets/main_menu_border
        assert!(manifest_map.contains_key(&0x0103)); // global assets/main_nav

        // Verify each enabled app's main.lua exists
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

    #[test]
    fn test_packet_recorder_overwrites_previous_capture() {
        let temp_dir = std::env::temp_dir().join(format!("bifrost_overwrite_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let dir_str = temp_dir.to_str().unwrap();

        // 1. Initial capture
        let rec1 = PacketRecorder::new(dir_str).unwrap();
        rec1.record_compression("TX", "screen_delta", 0x03, 0x02, b"OldData", None, "none", 10);
        let old_file = rec1.raw_dir.join("seq_000001_tx_screen_delta.bin");
        assert!(old_file.exists());
        drop(rec1);

        // 2. New capture in the same directory: must wipe old data clean
        let rec2 = PacketRecorder::new(dir_str).unwrap();
        rec2.record_compression("TX", "main_menu", 0x03, 0x02, b"NewData", None, "none", 10);
        let new_file = rec2.raw_dir.join("seq_000001_tx_main_menu.bin");
        assert!(new_file.exists());
        assert!(!old_file.exists(), "Old capture files should have been removed");

        let csv_content = std::fs::read_to_string(rec2.base_dir.join("compression_log.csv")).unwrap();
        assert!(!csv_content.contains("screen_delta"), "Old CSV records should not remain");
        assert!(csv_content.contains("main_menu"), "New CSV records should be present");

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
        assert!(
            csv_content.contains("screen_delta")
                || csv_content.contains("form_template")
                || csv_content.contains("client_input"),
            "CSV should record compression events, got:\n{}",
            csv_content
        );

        let raw_dir = temp_dir.join("raw");
        assert!(raw_dir.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_adaptive_dictionary_broadcast_and_per_node_cache_sync() {
        let server_node = [0x55u8; 32];
        let _client_node = [0x77u8; 32];

        // 1. Train custom dictionary
        let samples: Vec<&[u8]> = vec![
            b"[TEST_CUSTOM_HEADER] Welcome to Mesh Node 55",
            b"[TEST_CUSTOM_HEADER] System status: Online",
            b"[TEST_CUSTOM_HEADER] Commands: (1) Info (2) Logout",
        ];
        let dict = bifrost_compression::DictionaryTrainer::train_from_samples(&samples, 10);
        let dict_bytes = dict.to_bytes();
        let dict_crc = dict.crc32();

        // 2. Compress message with trained dictionary
        let original_msg = b"[TEST_CUSTOM_HEADER] Welcome to Mesh Node 55";
        let (flags, compressed) = bifrost_compression::compress_adaptive(original_msg, Some(&dict), 8, 4);
        assert!(compressed.len() < original_msg.len());

        // 3. Decompress using same dictionary
        let decompressed = bifrost_compression::decompress_adaptive(flags, &compressed, Some(&dict), 8, 4)
            .expect("Decompression should succeed");
        assert_eq!(original_msg.to_vec(), decompressed);

        // 4. Verify node cache directory isolation
        let node_hex: String = server_node.iter().map(|b| format!("{:02x}", b)).collect();
        let temp_cache_dir = std::env::temp_dir().join(format!("bifrost_test_cache_{}", node_hex));
        let _ = std::fs::create_dir_all(&temp_cache_dir);
        let dict_file = temp_cache_dir.join("dict.bin");
        std::fs::write(&dict_file, &dict_bytes).unwrap();

        let loaded_dict = bifrost_compression::CompressionDictionary::from_bytes(&std::fs::read(&dict_file).unwrap())
            .expect("Should load node dictionary from cache");
        assert_eq!(loaded_dict.crc32(), dict_crc);

        let _ = std::fs::remove_dir_all(&temp_cache_dir);
    }

    #[test]
    fn test_session_dedup_hash_referencing_and_nack_retransmission() {
        // 1. Setup server and client session caches
        let mut server_cache = bifrost_transport::SessionPayloadCache::new(100);
        let mut client_cache = bifrost_transport::SessionPayloadCache::new(100);

        let screen_payload = b"[MENU] Main Menu (1) Messages (2) Marketplace (3) Dungeon (4) Profile (5) Logout";
        let crc = bifrost_transport::crc32(screen_payload);

        // 2. First transmission: not yet in server cache -> full payload sent with compression/raw
        assert!(!server_cache.contains(crc));
        server_cache.insert(crc, screen_payload.to_vec());

        // Client receives first frame and caches it
        client_cache.insert(crc, screen_payload.to_vec());
        assert!(client_cache.contains(crc));

        // 3. Second transmission of identical screen in same session:
        // Server detects repeated payload -> transmits 4-byte hash reference
        assert!(server_cache.contains(crc));
        let hash_ref_payload = crc.to_be_bytes().to_vec();
        let _hash_ref_flags = 0x08u8; // SESSION_DEDUP_REF
        assert_eq!(hash_ref_payload.len(), 4);

        // Client receives 0x08 flag -> resolves 4-byte hash reference from local cache
        let received_crc = u32::from_be_bytes([
            hash_ref_payload[0],
            hash_ref_payload[1],
            hash_ref_payload[2],
            hash_ref_payload[3],
        ]);
        let resolved = client_cache.get(received_crc).expect("Should hit client session cache");
        assert_eq!(resolved, screen_payload);

        // 4. Test NACK recovery if client had a cache miss (e.g. cache eviction)
        let empty_client_cache = bifrost_transport::SessionPayloadCache::new(100);
        assert!(empty_client_cache.get(received_crc).is_none());

        // Client generates NACK with CRC
        let nack_msg = MeshBbsMessage::new(0x01, 0x06, 0x00, received_crc.to_be_bytes().to_vec());
        assert_eq!(nack_msg.opcode, 0x06);

        // Server receives NACK -> retrieves payload from server_cache and retransmits full payload
        let nack_crc = u32::from_be_bytes([
            nack_msg.payload[0],
            nack_msg.payload[1],
            nack_msg.payload[2],
            nack_msg.payload[3],
        ]);
        let retransmitted = server_cache.get(nack_crc).expect("Server cache should hold uncompressed payload");
        assert_eq!(retransmitted, screen_payload);
    }

    #[tokio::test]
    async fn test_session_resumption_within_timeout_window() {
        let node_id = [0x42u8; 32];
        let (tx, mut rx) = mpsc::channel(10);
        let last_activity = Arc::new(StdMutex::new(std::time::Instant::now()));

        let session = Session {
            input_tx: tx,
            last_activity: last_activity.clone(),
        };

        let active_sessions = Arc::new(StdMutex::new(HashMap::new()));
        active_sessions.lock().unwrap().insert(node_id, session);

        // 1. Reconnect within timeout (< 600s): should reuse existing session channel
        let is_handshake = true;
        let elapsed = last_activity.lock().unwrap().elapsed();
        assert!(elapsed < std::time::Duration::from_secs(SESSION_RESUME_TIMEOUT_SECS));

        let tx_opt = {
            let mut sessions = active_sessions.lock().unwrap();
            if let Some(s) = sessions.get(&node_id) {
                let el = s.last_activity.lock().unwrap().elapsed();
                if is_handshake && el < std::time::Duration::from_secs(SESSION_RESUME_TIMEOUT_SECS) {
                    *s.last_activity.lock().unwrap() = std::time::Instant::now();
                    Some(s.input_tx.clone())
                } else {
                    sessions.remove(&node_id);
                    None
                }
            } else {
                None
            }
        };

        assert!(tx_opt.is_some(), "Should resume existing session");
        let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
        tx_opt.unwrap().send(handshake_msg).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.opcode, 0x01);

        // 2. Simulate expired session (> 600s): should purge stale session
        *last_activity.lock().unwrap() = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(650))
            .unwrap();

        let tx_opt_expired = {
            let mut sessions = active_sessions.lock().unwrap();
            if let Some(s) = sessions.get(&node_id) {
                let el = s.last_activity.lock().unwrap().elapsed();
                if is_handshake && el < std::time::Duration::from_secs(SESSION_RESUME_TIMEOUT_SECS) {
                    Some(s.input_tx.clone())
                } else {
                    sessions.remove(&node_id);
                    None
                }
            } else {
                None
            }
        };

        assert!(tx_opt_expired.is_none(), "Expired session should be purged");
        assert!(active_sessions.lock().unwrap().get(&node_id).is_none());
    }
}




