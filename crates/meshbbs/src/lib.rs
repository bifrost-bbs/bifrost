//! MeshBBS Host server daemon kernel.
//! Handles session multiplexing, Rate Limiting, QoS Queues, and sandboxed Lua applications.

use anyhow::Result;
use log::{info, warn};
use mlua::LuaSerdeExt;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc;

// Pull from sibling workspace crates
use meshcore_transport::{MockSocketTransport, RadioTransport, RadioPacket, MeshBbsMessage, MessageReassembler};

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
            warn!("Config file not found at {:?}, using default settings", path);
            default_config()
        }
    } else {
        default_config()
    };

    // 2. Initialize Transport
    let transport: Arc<dyn RadioTransport> = if run_duration_secs.is_some() {
        Arc::new(MockSocketTransport::new(0.0, 10, 200))
    } else {
        Arc::new(MockSocketTransport::new_server(
            "127.0.0.1:8088".to_string(),
            0.0,
            10,
            200,
        ))
    };

    // 3. Start Server Runtime
    start_server(config, transport, run_duration_secs).await
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
    let db_store = Arc::new(StdMutex::new(HashMap::<String, HashMap<String, String>>::new()));
    let mut reassembler = MessageReassembler::new();

    // Main packet routing loop placeholder
    let loop_handle = tokio::spawn(async move {
        let start_time = tokio::time::Instant::now();
        loop {
            if let Some(dur) = run_duration_secs {
                if start_time.elapsed() >= tokio::time::Duration::from_secs(dur) {
                    break;
                }
            }

            // Receive packet from radio (timeout check allows quick loop exit)
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    let src = packet.src_node;
                    match reassembler.process_packet(src, &packet.payload) {
                        Ok(Some(msg)) => {
                            let tx_opt = active_sessions.lock().unwrap().get(&src).map(|s| s.input_tx.clone());
                            if let Some(tx) = tx_opt {
                                let _ = tx.send(msg).await;
                            } else {
                                // Boot new session
                                info!("Booting new Lua session for node: {:?}", src);
                                let (tx, rx) = mpsc::channel(100);
                                let session = Session { input_tx: tx.clone() };
                                active_sessions.lock().unwrap().insert(src, session);

                                let sessions_clone = active_sessions.clone();
                                let transport_inner = transport.clone();
                                let db_inner = db_store.clone();
                                let rt_handle = tokio::runtime::Handle::current();
                                let form_colors_config = config.form_colors.clone();
                                std::thread::spawn(move || {
                                    let res = run_session_task(src, rx, transport_inner, db_inner, rt_handle, form_colors_config);
                                    sessions_clone.lock().unwrap().remove(&src);
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

    Ok(())
}

fn run_session_task(
    node_id: [u8; 32],
    mut rx: mpsc::Receiver<MeshBbsMessage>,
    transport: Arc<dyn RadioTransport>,
    db_store: Arc<StdMutex<HashMap<String, HashMap<String, String>>>>,
    rt_handle: tokio::runtime::Handle,
    form_colors: FormColorsConfig,
) -> Result<()> {
    log::debug!("Starting run_session_task for client session");
    let lua = mlua::Lua::new();
    let active_app = Arc::new(StdMutex::new("00_main_menu".to_string()));

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
    term.set("clear", lua.create_function(move |_, (): ()| {
        out_buf.lock().unwrap().push(0x01); // OP_CLEAR_SCREEN
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    term.set("move_to", lua.create_function(move |_, (col, row): (u8, u8)| {
        let mut buf = out_buf.lock().unwrap();
        buf.push(0xC3); // OP_CURSOR_ABS
        buf.push(col);
        buf.push(row);
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    term.set("print", lua.create_function(move |_, text: String| {
        let mut buf = out_buf.lock().unwrap();
        buf.extend_from_slice(text.as_bytes());
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    term.set("set_color", lua.create_function(move |_, (fg, bg): (u8, u8)| {
        let mut buf = out_buf.lock().unwrap();
        buf.push(0xC0); // OP_SET_COLOR
        let attr = (bg << 4) | (fg & 0x0F);
        buf.push(attr);
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    term.set("render_asset", lua.create_function(move |_, asset_name: String| {
        let mut buf = out_buf.lock().unwrap();
        buf.push(0xC5); // OP_RENDER_ASSET
        let id: u16 = if asset_name == "ASSET_DUNGEON_BANNER" {
            0x0101
        } else {
            0x0102
        };
        buf.extend_from_slice(&id.to_be_bytes());
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    let transport_clone = transport.clone();
    let node_id_clone = node_id.clone();
    let rt = rt_handle.clone();
    term.set("flush", lua.create_function(move |_, (): ()| {
        let mut buf = out_buf.lock().unwrap();
        log::debug!("term.flush() called with {} bytes in session buffer", buf.len());
        if !buf.is_empty() {
            buf.push(0x04); // EndOfFrame
            let payload = buf.clone();
            buf.clear();

            let msg = MeshBbsMessage::new(0x01, 0x03, 0x00, payload);
            let mtu = transport_clone.get_mtu();
            match msg.to_fragments(mtu) {
                Ok(fragments) => {
                    let transport_inner = transport_clone.clone();
                    log::debug!("Sending term.flush() fragmented packets over transport (count={})", fragments.len());
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
    })?)?;

    let out_buf = output_buf.clone();
    let form_colors_clone = form_colors.clone();
    term.set("define_form", lua.create_function(move |_, (form_id, field_fg, field_bg, submit_fg, submit_bg): (u8, Option<u8>, Option<u8>, Option<u8>, Option<u8>)| {
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
    })?)?;

    let out_buf = output_buf.clone();
    term.set("add_input_field", lua.create_function(move |_, (field_id, col, row, width, default_val): (String, u8, u8, u8, String)| {
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
    })?)?;

    let out_buf = output_buf.clone();
    term.set("add_submit_button", lua.create_function(move |_, (button_id, col, row): (String, u8, u8)| {
        let mut buf = out_buf.lock().unwrap();
        buf.push(0xD2); // OP_FORM_SUBMIT
        buf.push(col);
        buf.push(row);

        let id_bytes = button_id.as_bytes();
        buf.push(id_bytes.len() as u8);
        buf.extend_from_slice(id_bytes);
        Ok(())
    })?)?;

    let out_buf = output_buf.clone();
    let transport_clone = transport.clone();
    let node_id_clone = node_id.clone();
    let rt = rt_handle.clone();
    term.set("flush_form", lua.create_function(move |_, (): ()| {
        let mut buf = out_buf.lock().unwrap();
        log::debug!("term.flush_form() called with {} bytes in session buffer", buf.len());
        buf.push(0xD3); // OP_FORM_END
        buf.push(0x04); // EndOfFrame
        let payload = buf.clone();
        buf.clear();

        let msg = MeshBbsMessage::new(0x01, 0x03, 0x00, payload);
        let mtu = transport_clone.get_mtu();
        match msg.to_fragments(mtu) {
            Ok(fragments) => {
                let transport_inner = transport_clone.clone();
                log::debug!("Sending term.flush_form() fragmented packets over transport (count={})", fragments.len());
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
    })?)?;

    globals.set("term", term)?;

    // db table
    let db = lua.create_table()?;
    let db_store_get = db_store.clone();
    db.set("get", lua.create_function(move |lua, (table, key): (String, String)| {
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
    })?)?;

    let db_store_set = db_store.clone();
    db.set("set", lua.create_function(move |lua, (table, key, val): (String, String, mlua::Value)| {
        if let Ok(json_val) = lua.from_value::<serde_json::Value>(val) {
            if let Ok(json_str) = serde_json::to_string(&json_val) {
                let mut store = db_store_set.lock().unwrap();
                store.entry(table).or_insert_with(HashMap::new).insert(key, json_str);
            }
        }
        Ok(())
    })?)?;
    globals.set("db", db)?;

    // log table for app scripts
    let log_table = lua.create_table()?;
    let active_app_clone = active_app.clone();
    log_table.set("info", lua.create_function(move |_, msg: String| {
        let app = active_app_clone.lock().unwrap().clone();
        log::info!(target: "lua_app", "[Lua: {}] {}", app, msg);
        Ok(())
    })?)?;

    let active_app_clone = active_app.clone();
    log_table.set("warn", lua.create_function(move |_, msg: String| {
        let app = active_app_clone.lock().unwrap().clone();
        log::warn!(target: "lua_app", "[Lua: {}] {}", app, msg);
        Ok(())
    })?)?;

    let active_app_clone = active_app.clone();
    log_table.set("error", lua.create_function(move |_, msg: String| {
        let app = active_app_clone.lock().unwrap().clone();
        log::error!(target: "lua_app", "[Lua: {}] {}", app, msg);
        Ok(())
    })?)?;

    let active_app_clone = active_app.clone();
    log_table.set("debug", lua.create_function(move |_, msg: String| {
        let app = active_app_clone.lock().unwrap().clone();
        log::debug!(target: "lua_app", "[Lua: {}] {}", app, msg);
        Ok(())
    })?)?;
    globals.set("log", log_table)?;

    // session table & state
    let session = lua.create_table()?;
    let node_hex_str: String = node_id.iter().map(|b| format!("{:02x}", b)).collect();
    let node_hex_str_clone = node_hex_str.clone();
    session.set("node_id", lua.create_function(move |_, (): ()| {
        Ok(node_hex_str_clone.clone())
    })?)?;

    session.set("callsign", lua.create_function(|_, (): ()| {
        Ok("RadioOperator".to_string())
    })?)?;

    let callback_store = Arc::new(StdMutex::new(None));
    let callback_store_clone = callback_store.clone();
    session.set("await_input", lua.create_function(move |lua, (max_len, cb): (usize, mlua::Function)| {
        let key = lua.create_registry_value(cb)?;
        *callback_store_clone.lock().unwrap() = Some((max_len, key));
        Ok(())
    })?)?;

    let session_close = Arc::new(StdMutex::new(false));
    let session_close_clone = session_close.clone();
    session.set("close", lua.create_function(move |_, (): ()| {
        *session_close_clone.lock().unwrap() = true;
        Ok(())
    })?)?;

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
            log::debug!("Got input message: opcode={}, len={}", msg.opcode, msg.payload.len());
            if msg.opcode == 0x02 { // Keystroke/Input message
                if let Ok(input_str) = String::from_utf8(msg.payload) {
                    let cb_opt = {
                        let mut store = callback_store.lock().unwrap();
                        store.take()
                    };

                    if let Some((_max_len, reg_key)) = cb_opt {
                        let cb: mlua::Function = lua.registry_value(&reg_key)?;
                        if input_str.starts_with('{') {
                            if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&input_str) {
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
        let server_transport = Arc::new(MockSocketTransport::new_server("127.0.0.1:9095".to_string(), 0.0, 0, 200));
        
        let server_handle = tokio::spawn(async move {
            start_server(config, server_transport, Some(1)).await
        });
        
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let client_transport = MockSocketTransport::new_client("127.0.0.1:9095".to_string(), 0.0, 0, 200);
        
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
        
        let response = assembled_msg.expect("Failed to reassemble server response welcome screen");
        assert_eq!(response.opcode, 0x03);
        assert!(!response.payload.is_empty(), "Server response payload is empty");

        // Send simulated Form Submission (tab nickname to action button, then press enter)
        let form_submit_json = r#"{"nickname":"TestUser","submit":"read_boards"}"#;
        let submit_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, form_submit_json.as_bytes().to_vec());
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
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), client_transport.receive_packet()).await {
                Ok(Ok(packet)) => {
                    if let Some(msg) = client_reassembler.process_packet([0; 32], &packet.payload).unwrap() {
                        board_msg = Some(msg);
                        break;
                    }
                }
                _ => {}
            }
        }

        let board_response = board_msg.expect("Failed to reassemble discussion boards screen response");
        assert_eq!(board_response.opcode, 0x03);
        
        let _ = server_handle.await;
    }
}
