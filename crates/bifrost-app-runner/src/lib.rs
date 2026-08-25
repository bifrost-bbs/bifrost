//! Standalone Bifrost Lua App Test Runner Engine.
//!
//! Provides an isolated, host-level Lua 5.4 execution environment with complete
//! mocks for `term`, `session`, `db`, `log`, and `http` APIs, plus virtual 80x25
//! ANSI screen buffering for testing without a live BBS or radio mesh network.

use anyhow::{Context, Result};
use bifrost_bbs::db::DatabaseStore;
use bifrost_bbs::network::BbsNetworkRegistryManager;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use mlua::{Lua, LuaSerdeExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 80x25 Virtual Terminal Character Cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: u8,
    pub bg: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: 7, // Light Gray
            bg: 0, // Black
        }
    }
}

/// 80x25 Virtual Screen Buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualScreen {
    pub cells: Vec<Vec<Cell>>,
    pub cursor_col: u8,
    pub cursor_row: u8,
    pub current_fg: u8,
    pub current_bg: u8,
}

impl VirtualScreen {
    pub fn new() -> Self {
        let mut screen = Self {
            cells: vec![vec![Cell::default(); 80]; 25],
            cursor_col: 1,
            cursor_row: 1,
            current_fg: 7,
            current_bg: 0,
        };
        screen.clear();
        screen
    }

    pub fn clear(&mut self) {
        for row in &mut self.cells {
            for cell in row.iter_mut() {
                *cell = Cell {
                    ch: ' ',
                    fg: self.current_fg,
                    bg: self.current_bg,
                };
            }
        }
        self.cursor_col = 1;
        self.cursor_row = 1;
    }

    pub fn move_to(&mut self, col: u8, row: u8) {
        self.cursor_col = col.clamp(1, 80);
        self.cursor_row = row.clamp(1, 25);
    }

    pub fn set_color(&mut self, fg: u8, bg: u8) {
        self.current_fg = fg.clamp(0, 15);
        self.current_bg = bg.clamp(0, 15);
    }

    pub fn print_str(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                self.cursor_col = 1;
                if self.cursor_row < 25 {
                    self.cursor_row += 1;
                }
            } else if ch == '\r' {
                self.cursor_col = 1;
            } else if ch == '\t' {
                let next_tab = ((self.cursor_col - 1) / 4 + 1) * 4 + 1;
                self.cursor_col = next_tab.min(80);
            } else {
                if self.cursor_col > 80 {
                    self.cursor_col = 1;
                    if self.cursor_row < 25 {
                        self.cursor_row += 1;
                    }
                }
                let c = (self.cursor_col - 1) as usize;
                let r = (self.cursor_row - 1) as usize;
                if r < 25 && c < 80 {
                    self.cells[r][c] = Cell {
                        ch,
                        fg: self.current_fg,
                        bg: self.current_bg,
                    };
                    self.cursor_col += 1;
                }
            }
        }
    }

    pub fn render_to_plain_text(&self) -> String {
        let mut out = String::with_capacity(80 * 25 + 25);
        for row in &self.cells {
            let row_str: String = row.iter().map(|c| c.ch).collect();
            out.push_str(row_str.trim_end());
            out.push('\n');
        }
        out
    }

    pub fn ansi_to_crossterm_color(code: u8) -> Color {
        match code {
            0 => Color::Black,
            1 => Color::DarkBlue,
            2 => Color::DarkGreen,
            3 => Color::DarkCyan,
            4 => Color::DarkRed,
            5 => Color::DarkMagenta,
            6 => Color::DarkYellow,
            7 => Color::Grey,
            8 => Color::DarkGrey,
            9 => Color::Blue,
            10 => Color::Green,
            11 => Color::Cyan,
            12 => Color::Red,
            13 => Color::Magenta,
            14 => Color::Yellow,
            15 => Color::White,
            _ => Color::Reset,
        }
    }

    pub fn draw_to_terminal<W: Write>(&self, w: &mut W, title: &str) -> Result<()> {
        execute!(w, MoveTo(0, 0))?;

        // Render top banner
        execute!(
            w,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Cyan)
        )?;
        let header = format!(" ╔══ BIFROST APP RUNNER :: {} ══", title);
        write!(w, "{:<80}", header)?;
        execute!(w, ResetColor)?;

        // Render 25 virtual screen rows
        for (r, row) in self.cells.iter().enumerate() {
            execute!(w, MoveTo(0, (r + 1) as u16))?;
            let mut last_fg = 255;
            let mut last_bg = 255;

            for cell in row {
                if cell.fg != last_fg {
                    execute!(w, SetForegroundColor(Self::ansi_to_crossterm_color(cell.fg)))?;
                    last_fg = cell.fg;
                }
                if cell.bg != last_bg {
                    execute!(w, SetBackgroundColor(Self::ansi_to_crossterm_color(cell.bg)))?;
                    last_bg = cell.bg;
                }
                write!(w, "{}", cell.ch)?;
            }
        }

        // Render bottom status bar
        execute!(w, MoveTo(0, 26))?;
        execute!(
            w,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::DarkGrey)
        )?;
        write!(
            w,
            "{:<80}",
            " [Tab/Shift+Tab] Focus | [Enter] Submit | [Arrows] Navigate | [Esc/Ctrl+C] Quit"
        )?;
        execute!(w, ResetColor)?;

        w.flush()?;
        Ok(())
    }
}

/// Interactive Form Field Definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub id: String,
    pub col: u8,
    pub row: u8,
    pub width: u8,
    pub height: u8,
    pub value: String,
    pub is_submit: bool,
    pub hotkey: Option<char>,
}

/// Active Form State.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    pub active: bool,
    pub form_id: u8,
    pub fields: Vec<FormField>,
    pub active_idx: usize,
    pub field_fg: u8,
    pub field_bg: u8,
    pub submit_fg: u8,
    pub submit_bg: u8,
}

impl Default for FormState {
    fn default() -> Self {
        Self {
            active: false,
            form_id: 0,
            fields: Vec::new(),
            active_idx: 0,
            field_fg: 15,
            field_bg: 4,
            submit_fg: 0,
            submit_bg: 7,
        }
    }
}

/// Manifest schema for `manifest.toml`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppManifest {
    pub app: AppMetadata,
    #[serde(default)]
    pub assets: Vec<AssetDeclaration>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AppMetadata {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_entry")]
    pub entry_point: String,
    #[serde(default)]
    pub admin_only: Option<bool>,
    #[serde(default)]
    pub required_permission: Option<String>,
    #[serde(default)]
    pub hotkey: Option<String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_entry() -> String {
    "main.lua".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AssetDeclaration {
    pub name: String,
    pub path: String,
}

/// Configuration settings for the App Runner execution.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub app_dir: PathBuf,
    pub user_nickname: String,
    pub user_node_id: String,
    pub is_admin: bool,
    pub headless: bool,
    pub db_path: Option<String>,
    pub initial_submissions: Vec<serde_json::Value>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            app_dir: PathBuf::from("."),
            user_nickname: "DevOperator".to_string(),
            user_node_id: "0101010101010101010101010101010101010101010101010101010101010101"
                .to_string(),
            is_admin: true,
            headless: false,
            db_path: None,
            initial_submissions: Vec::new(),
        }
    }
}

/// Standalone Lua BBS Application Test Runner.
pub struct AppRunner {
    pub config: RunnerConfig,
    pub manifest: AppManifest,
    pub screen: Arc<Mutex<VirtualScreen>>,
    pub form: Arc<Mutex<FormState>>,
    pub db_store: DatabaseStore,
    pub assets: Arc<HashMap<String, String>>,
    pub closed: Arc<Mutex<bool>>,
    pub pending_callback: Arc<Mutex<Option<mlua::RegistryKey>>>,
}

impl AppRunner {
    pub fn new(config: RunnerConfig) -> Result<Self> {
        let manifest_path = config.app_dir.join("manifest.toml");
        if !manifest_path.exists() {
            anyhow::bail!(
                "manifest.toml not found in application directory: {:?}",
                config.app_dir
            );
        }

        let manifest_str = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("Failed to read {:?}", manifest_path))?;
        let manifest: AppManifest = toml::from_str(&manifest_str)
            .with_context(|| format!("Failed to parse manifest.toml in {:?}", config.app_dir))?;

        // Load declared static assets
        let mut assets = HashMap::new();
        for decl in &manifest.assets {
            let asset_file = config.app_dir.join(&decl.path);
            if asset_file.exists() {
                if let Ok(content) = std::fs::read_to_string(&asset_file) {
                    assets.insert(decl.name.clone(), content);
                }
            }
        }

        // Initialize SQLite DB (either in-memory or on disk)
        let db_store = if let Some(ref path) = config.db_path {
            DatabaseStore::new(path)?
        } else {
            DatabaseStore::new_in_memory()?
        };

        Ok(Self {
            config,
            manifest,
            screen: Arc::new(Mutex::new(VirtualScreen::new())),
            form: Arc::new(Mutex::new(FormState::default())),
            db_store,
            assets: Arc::new(assets),
            closed: Arc::new(Mutex::new(false)),
            pending_callback: Arc::new(Mutex::new(None)),
        })
    }

    /// Initializes sandboxed Lua environment with all BBS host APIs.
    pub fn setup_lua(&self) -> Result<Lua> {
        let lua = Lua::new();

        // 1. Term Global API
        let screen_ref = self.screen.clone();
        let form_ref = self.form.clone();
        let assets_ref = self.assets.clone();
        let term_tbl = lua.create_table()?;

        let scr = screen_ref.clone();
        term_tbl.set(
            "clear",
            lua.create_function(move |_, (): ()| {
                scr.lock().unwrap().clear();
                Ok(())
            })?,
        )?;

        let scr = screen_ref.clone();
        let move_to_fn = lua.create_function(move |_, (col, row): (u8, u8)| {
            scr.lock().unwrap().move_to(col, row);
            Ok(())
        })?;
        term_tbl.set("move_to", move_to_fn.clone())?;
        term_tbl.set("set_cursor", move_to_fn)?;

        let scr = screen_ref.clone();
        term_tbl.set(
            "set_color",
            lua.create_function(move |_, (fg, bg): (u8, u8)| {
                scr.lock().unwrap().set_color(fg, bg);
                Ok(())
            })?,
        )?;

        let scr = screen_ref.clone();
        term_tbl.set(
            "print",
            lua.create_function(move |_, text: String| {
                scr.lock().unwrap().print_str(&text);
                Ok(())
            })?,
        )?;

        let scr = screen_ref.clone();
        let assets_for_render = assets_ref.clone();
        term_tbl.set(
            "render_asset",
            lua.create_function(move |_, name: String| {
                let clean_name = name.split('/').last().unwrap_or(&name);
                if let Some(content) = assets_for_render.get(clean_name) {
                    scr.lock().unwrap().print_str(content);
                } else {
                    log::warn!("Asset '{}' not found in assets map", name);
                }
                Ok(())
            })?,
        )?;

        let scr = screen_ref.clone();
        let assets_for_template = assets_ref.clone();
        term_tbl.set(
            "render_template",
            lua.create_function(move |_, (name, params): (String, mlua::Value)| {
                let clean_name = name.split('/').last().unwrap_or(&name);
                if let Some(template) = assets_for_template.get(clean_name) {
                    let mut rendered = template.clone();
                    match params {
                        mlua::Value::Table(tbl) => {
                            for pair in tbl.pairs::<mlua::Value, String>() {
                                if let Ok((k, v)) = pair {
                                    let key_str = match k {
                                        mlua::Value::Integer(i) => format!("{{{}}}", i),
                                        mlua::Value::String(s) => format!("{{{}}}", s.to_str()?),
                                        _ => continue,
                                    };
                                    rendered = rendered.replace(&key_str, &v);
                                }
                            }
                        }
                        mlua::Value::String(s) => {
                            rendered = rendered.replacen("%s", s.to_str()?, 1);
                        }
                        _ => {}
                    }
                    scr.lock().unwrap().print_str(&rendered);
                }
                Ok(())
            })?,
        )?;

        let scr = screen_ref.clone();
        let assets_for_menu = assets_ref.clone();
        let frm = form_ref.clone();
        term_tbl.set(
            "render_menu",
            lua.create_function(move |_, (name, _mask): (String, Option<mlua::Value>)| {
                let clean_name = name.split('/').last().unwrap_or(&name);
                if let Some(csv_content) = assets_for_menu.get(clean_name) {
                    let mut lines = csv_content.lines();
                    let mut form_id = 10u8;
                    let mut form_state = frm.lock().unwrap();

                    for line in lines.by_ref() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("# form_id=") {
                            if let Ok(id) = trimmed[10..].trim().parse::<u8>() {
                                form_id = id;
                            }
                            continue;
                        }
                        if trimmed.starts_with('#') || trimmed.is_empty() {
                            continue;
                        }
                        let parts: Vec<&str> = trimmed.split(',').map(|s| s.trim()).collect();
                        if parts.len() >= 5 {
                            let tag = parts[0];
                            let id = parts[1];
                            let label = parts[2];
                            let col = parts[3].parse::<u8>().unwrap_or(2);
                            let row = parts[4].parse::<u8>().unwrap_or(10);
                            let hotkey = parts.get(5).and_then(|k| k.chars().next());

                            form_state.fields.push(FormField {
                                id: if id.is_empty() { tag.to_string() } else { id.to_string() },
                                col,
                                row,
                                width: label.len() as u8,
                                height: 1,
                                value: label.to_string(),
                                is_submit: true,
                                hotkey,
                            });

                            let mut screen = scr.lock().unwrap();
                            screen.move_to(col, row);
                            screen.print_str(&format!("[{}]", label));
                        }
                    }
                    form_state.active = true;
                    form_state.form_id = form_id;
                }
                Ok(())
            })?,
        )?;

        let frm = form_ref.clone();
        term_tbl.set(
            "define_form",
            lua.create_function(
                move |_, (form_id, f_fg, f_bg, s_fg, s_bg): (u8, Option<u8>, Option<u8>, Option<u8>, Option<u8>)| {
                    let mut state = frm.lock().unwrap();
                    state.active = true;
                    state.form_id = form_id;
                    state.fields.clear();
                    state.active_idx = 0;
                    if let Some(fg) = f_fg { state.field_fg = fg; }
                    if let Some(bg) = f_bg { state.field_bg = bg; }
                    if let Some(fg) = s_fg { state.submit_fg = fg; }
                    if let Some(bg) = s_bg { state.submit_bg = bg; }
                    Ok(())
                },
            )?,
        )?;

        let frm = form_ref.clone();
        let scr = screen_ref.clone();
        term_tbl.set(
            "add_input_field",
            lua.create_function(
                move |_, (id, col, row, width, default_val): (String, u8, u8, u8, Option<String>)| {
                    let val = default_val.unwrap_or_default();
                    frm.lock().unwrap().fields.push(FormField {
                        id,
                        col,
                        row,
                        width,
                        height: 1,
                        value: val.clone(),
                        is_submit: false,
                        hotkey: None,
                    });
                    let mut s = scr.lock().unwrap();
                    s.move_to(col, row);
                    let padded = format!("{:<width$}", val, width = width as usize);
                    s.print_str(&padded);
                    Ok(())
                },
            )?,
        )?;

        let frm = form_ref.clone();
        let scr = screen_ref.clone();
        term_tbl.set(
            "add_multiline_field",
            lua.create_function(
                move |_, (id, col, row, width, height, default_val): (String, u8, u8, u8, u8, Option<String>)| {
                    let val = default_val.unwrap_or_default();
                    frm.lock().unwrap().fields.push(FormField {
                        id,
                        col,
                        row,
                        width,
                        height,
                        value: val.clone(),
                        is_submit: false,
                        hotkey: None,
                    });
                    let mut s = scr.lock().unwrap();
                    s.move_to(col, row);
                    s.print_str(&val);
                    Ok(())
                },
            )?,
        )?;

        let frm = form_ref.clone();
        term_tbl.set(
            "add_submit_button",
            lua.create_function(move |_, (id, col, row): (String, u8, u8)| {
                frm.lock().unwrap().fields.push(FormField {
                    id,
                    col,
                    row,
                    width: 10,
                    height: 1,
                    value: String::new(),
                    is_submit: true,
                    hotkey: None,
                });
                Ok(())
            })?,
        )?;

        term_tbl.set(
            "flush_form",
            lua.create_function(|_, (): ()| Ok(()))?,
        )?;
        term_tbl.set(
            "flush",
            lua.create_function(|_, (): ()| Ok(()))?,
        )?;

        lua.globals().set("term", term_tbl)?;

        // 2. Session Global API
        let session_tbl = lua.create_table()?;
        let node_id_val = self.config.user_node_id.clone();
        session_tbl.set(
            "node_id",
            lua.create_function(move |_, (): ()| Ok(node_id_val.clone()))?,
        )?;

        let nick_val = self.config.user_nickname.clone();
        session_tbl.set(
            "callsign",
            lua.create_function(move |_, (): ()| Ok(nick_val.clone()))?,
        )?;

        let pending_cb = self.pending_callback.clone();
        session_tbl.set(
            "await_input",
            lua.create_function(move |lua, (_max_len, callback): (u8, mlua::Function)| {
                let key = lua.create_registry_value(callback)?;
                *pending_cb.lock().unwrap() = Some(key);
                Ok(())
            })?,
        )?;

        let closed_flag = self.closed.clone();
        session_tbl.set(
            "close",
            lua.create_function(move |_, (): ()| {
                *closed_flag.lock().unwrap() = true;
                Ok(())
            })?,
        )?;

        let is_admin_val = self.config.is_admin;
        session_tbl.set(
            "has_permission",
            lua.create_function(move |_, perm: String| {
                if is_admin_val || perm == "read" || perm == "write" {
                    Ok(true)
                } else {
                    Ok(false)
                }
            })?,
        )?;

        session_tbl.set(
            "permissions",
            lua.create_function(move |lua, (): ()| {
                let perms = if is_admin_val {
                    vec!["admin", "read", "write"]
                } else {
                    vec!["read", "write"]
                };
                let tbl = lua.create_table()?;
                for (i, p) in perms.into_iter().enumerate() {
                    tbl.set(i + 1, p)?;
                }
                Ok(tbl)
            })?,
        )?;

        session_tbl.set(
            "get_menu_config",
            lua.create_function(|lua, (): ()| {
                let tbl = lua.create_table()?;
                tbl.set("title", "=== BIFROST LOCAL DEV ===")?;
                tbl.set("header_fg", 14)?;
                tbl.set("header_bg", 0)?;
                tbl.set("layout", "grid")?;
                tbl.set("start_col", 2)?;
                tbl.set("start_row", 10)?;
                tbl.set("col_width", 16)?;
                tbl.set("show_logout", true)?;
                Ok(tbl)
            })?,
        )?;

        session_tbl.set(
            "get_apps",
            lua.create_function(|lua, (): ()| {
                let tbl = lua.create_table()?;
                let app = lua.create_table()?;
                app.set("id", "starter")?;
                app.set("name", "Starter Demo")?;
                app.set("description", "Demo template app")?;
                tbl.set(1, app)?;
                Ok(tbl)
            })?,
        )?;

        let app_dir_for_include = self.config.app_dir.clone();
        session_tbl.set(
            "include",
            lua.create_function(move |lua, filename: String| {
                let path = if filename.ends_with(".lua") {
                    app_dir_for_include.join(&filename)
                } else {
                    app_dir_for_include.join(format!("{}.lua", filename))
                };
                if path.exists() {
                    let code = std::fs::read_to_string(&path)?;
                    let chunk = lua.load(&code).set_name(&filename);
                    let val: mlua::Value = chunk.eval()?;
                    Ok(val)
                } else {
                    Err(mlua::Error::RuntimeError(format!("Included file not found: {:?}", path)))
                }
            })?,
        )?;

        session_tbl.set(
            "load_app",
            lua.create_function(|_, app_name: String| {
                log::info!("session.load_app('{}') requested in local runner", app_name);
                Ok(())
            })?,
        )?;

        session_tbl.set(
            "time",
            lua.create_function(|_, (): ()| {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(secs)
            })?,
        )?;

        session_tbl.set(
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

        session_tbl.set(
            "is_network_enabled",
            lua.create_function(|_, (): ()| Ok(true))?,
        )?;

        let reg_mgr = BbsNetworkRegistryManager::new(".client_cache/runner_net_registry.json");
        session_tbl.set(
            "get_network_nodes",
            lua.create_function(move |lua, query: Option<String>| {
                let q = query.unwrap_or_default();
                let nodes = reg_mgr.search(&q);
                let tbl = lua.create_table()?;
                for (idx, node) in nodes.into_iter().enumerate() {
                    let node_tbl = lua.create_table()?;
                    node_tbl.set("node_id", node.node_id)?;
                    node_tbl.set("name", node.name)?;
                    node_tbl.set("callsign", node.callsign)?;
                    node_tbl.set("description", node.description)?;
                    node_tbl.set("region", node.location.region)?;
                    node_tbl.set("grid", node.location.grid)?;
                    node_tbl.set("lat", node.location.lat)?;
                    node_tbl.set("lon", node.location.lon)?;
                    node_tbl.set("contact", node.sysop.contact)?;
                    node_tbl.set("relay_enabled", node.capabilities.relay_enabled)?;
                    tbl.set(idx + 1, node_tbl)?;
                }
                Ok(tbl)
            })?,
        )?;

        session_tbl.set(
            "start_relay_session",
            lua.create_function(|_, target_node_id: String| {
                log::info!("Mock relay session to node: {}", target_node_id);
                Ok(true)
            })?,
        )?;

        lua.globals().set("session", session_tbl)?;

        // 3. Database Global API (`db`)
        let db_store_ref = self.db_store.clone();
        let db_tbl = lua.create_table()?;

        let db_for_get = db_store_ref.clone();
        db_tbl.set(
            "get",
            lua.create_function(move |lua, (table, key): (String, Option<mlua::Value>)| {
                let key_str = match key {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    Some(mlua::Value::Integer(i)) => i.to_string(),
                    _ => "all".to_string(),
                };
                if let Ok(Some(val)) = db_for_get.get(&table, &key_str) {
                    let json_val: serde_json::Value = serde_json::from_str(&val).unwrap_or(serde_json::Value::String(val));
                    let lua_val = lua.to_value(&json_val)?;
                    Ok(lua_val)
                } else {
                    Ok(mlua::Value::Nil)
                }
            })?,
        )?;

        let db_for_set = db_store_ref.clone();
        db_tbl.set(
            "set",
            lua.create_function(move |_, (table, key, val): (String, Option<mlua::Value>, mlua::Value)| {
                let key_str = match key {
                    Some(mlua::Value::String(s)) => s.to_str()?.to_string(),
                    Some(mlua::Value::Integer(i)) => i.to_string(),
                    _ => "all".to_string(),
                };
                match val {
                    mlua::Value::Nil => {
                        let _ = db_for_set.remove(&table, &key_str);
                    }
                    _ => {
                        let json_val: serde_json::Value = serde_json::to_value(&val).unwrap_or(serde_json::Value::Null);
                        let serialized = serde_json::to_string(&json_val).unwrap_or_default();
                        let _ = db_for_set.set(&table, &key_str, &serialized);
                    }
                }
                Ok(())
            })?,
        )?;

        let db_for_keys = db_store_ref;
        db_tbl.set(
            "keys",
            lua.create_function(move |lua, table: String| {
                let keys = db_for_keys.keys(&table).unwrap_or_default();
                let tbl = lua.create_table()?;
                for (idx, k) in keys.into_iter().enumerate() {
                    tbl.set(idx + 1, k)?;
                }
                Ok(tbl)
            })?,
        )?;

        lua.globals().set("db", db_tbl)?;

        // 4. Log Global API (`log`)
        let log_tbl = lua.create_table()?;
        log_tbl.set("info", lua.create_function(|_, msg: String| { log::info!("[APP] {}", msg); Ok(()) })?)?;
        log_tbl.set("warn", lua.create_function(|_, msg: String| { log::warn!("[APP] {}", msg); Ok(()) })?)?;
        log_tbl.set("error", lua.create_function(|_, msg: String| { log::error!("[APP] {}", msg); Ok(()) })?)?;
        log_tbl.set("debug", lua.create_function(|_, msg: String| { log::debug!("[APP] {}", msg); Ok(()) })?)?;
        lua.globals().set("log", log_tbl)?;

        // 5. HTTP Global API (`http`)
        let http_tbl = lua.create_table()?;
        http_tbl.set(
            "get_json",
            lua.create_function(|lua, url: String| {
                if let Ok(resp) = reqwest::blocking::get(&url) {
                    if let Ok(text) = resp.text() {
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&text) {
                            let val = lua.to_value(&json_val)?;
                            return Ok(val);
                        }
                    }
                }
                Ok(mlua::Value::Nil)
            })?,
        )?;
        lua.globals().set("http", http_tbl)?;

        Ok(lua)
    }

    /// Executes the application `on_start` hook.
    pub fn run_start(&self, lua: &Lua) -> Result<()> {
        let entry_file = self.config.app_dir.join(&self.manifest.app.entry_point);
        if !entry_file.exists() {
            anyhow::bail!("Entry point {:?} not found", entry_file);
        }

        let code = std::fs::read_to_string(&entry_file)?;
        let app_module: mlua::Table = lua.load(&code).set_name(&self.manifest.app.id).eval()?;

        if let Ok(on_start) = app_module.get::<_, mlua::Function>("on_start") {
            let session_tbl: mlua::Table = lua.globals().get("session")?;
            on_start.call::<_, ()>(session_tbl)?;
        }

        Ok(())
    }

    /// Invokes the pending input callback with a submission table.
    pub fn submit_input(&self, lua: &Lua, submission: serde_json::Value) -> Result<()> {
        let cb_key = {
            let mut guard = self.pending_callback.lock().unwrap();
            guard.take()
        };

        if let Some(key) = cb_key {
            let callback: mlua::Function = lua.registry_value(&key)?;
            let lua_val = lua.to_value(&submission)?;
            callback.call::<_, ()>(lua_val)?;
        }

        Ok(())
    }

    /// Runs interactive terminal loop using Crossterm.
    pub fn run_interactive(&self) -> Result<()> {
        let lua = self.setup_lua()?;
        self.run_start(&lua)?;

        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, Hide)?;

        let res = self.event_loop(&lua, &mut out);

        execute!(out, ResetColor, Show, LeaveAlternateScreen)?;
        disable_raw_mode()?;

        res
    }

    fn event_loop<W: Write>(&self, lua: &Lua, out: &mut W) -> Result<()> {
        let title = format!("{} v{}", self.manifest.app.name, self.manifest.app.version);
        let mut dirty = true;

        loop {
            if *self.closed.lock().unwrap() {
                break;
            }

            if dirty {
                self.screen.lock().unwrap().draw_to_terminal(out, &title)?;
                dirty = false;
            }

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                        KeyCode::Tab => {
                            let mut form = self.form.lock().unwrap();
                            if form.active && !form.fields.is_empty() {
                                form.active_idx = (form.active_idx + 1) % form.fields.len();
                                dirty = true;
                            }
                        }
                        KeyCode::BackTab => {
                            let mut form = self.form.lock().unwrap();
                            if form.active && !form.fields.is_empty() {
                                form.active_idx = if form.active_idx == 0 {
                                    form.fields.len() - 1
                                } else {
                                    form.active_idx - 1
                                };
                                dirty = true;
                            }
                        }
                        KeyCode::Enter => {
                            let form_opt = {
                                let form = self.form.lock().unwrap();
                                if form.active && !form.fields.is_empty() {
                                    Some((form.fields.clone(), form.active_idx))
                                } else {
                                    None
                                }
                            };

                            if let Some((fields, active_idx)) = form_opt {
                                let mut submission = serde_json::Map::new();
                                for (i, f) in fields.iter().enumerate() {
                                    if f.is_submit {
                                        if i == active_idx {
                                            submission.insert(
                                                "submit".to_string(),
                                                serde_json::Value::String(f.id.clone()),
                                            );
                                        }
                                    } else {
                                        submission.insert(
                                            f.id.clone(),
                                            serde_json::Value::String(f.value.clone()),
                                        );
                                    }
                                }

                                if !submission.contains_key("submit") {
                                    // Default submit to first button or active field
                                    if let Some(first_btn) = fields.iter().find(|f| f.is_submit) {
                                        submission.insert(
                                            "submit".to_string(),
                                            serde_json::Value::String(first_btn.id.clone()),
                                        );
                                    }
                                }

                                self.submit_input(lua, serde_json::Value::Object(submission))?;
                                dirty = true;
                            } else {
                                self.submit_input(lua, serde_json::Value::String("enter".to_string()))?;
                                dirty = true;
                            }
                        }
                        KeyCode::Backspace => {
                            let mut form = self.form.lock().unwrap();
                            if form.active && !form.fields.is_empty() {
                                let idx = form.active_idx;
                                if idx < form.fields.len() && !form.fields[idx].is_submit {
                                    form.fields[idx].value.pop();
                                    dirty = true;
                                }
                            }
                        }
                        KeyCode::Char(c) => {
                            let mut form = self.form.lock().unwrap();
                            if form.active && !form.fields.is_empty() {
                                let idx = form.active_idx;
                                if idx < form.fields.len() && !form.fields[idx].is_submit {
                                    form.fields[idx].value.push(c);
                                    dirty = true;
                                }
                            } else {
                                drop(form);
                                self.submit_input(lua, serde_json::Value::String(c.to_string()))?;
                                dirty = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_virtual_screen_basic_and_wrapping() {
        let mut screen = VirtualScreen::new();
        screen.set_color(14, 0);
        screen.move_to(2, 2);
        screen.print_str("Hello World!\nSecond Line");

        assert_eq!(screen.cells[1][1].ch, 'H');
        assert_eq!(screen.cells[1][1].fg, 14);
        assert_eq!(screen.cells[2][0].ch, 'S');

        let plain = screen.render_to_plain_text();
        assert!(plain.contains("Hello World!"));
        assert!(plain.contains("Second Line"));
    }

    #[test]
    fn test_app_runner_headless_execution() {
        let temp_dir = std::env::temp_dir().join(format!("bifrost_runner_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let manifest_content = r#"
[app]
id = "test_app"
name = "Test App"
version = "1.0.0"
entry_point = "main.lua"
"#;
        fs::write(temp_dir.join("manifest.toml"), manifest_content).unwrap();

        let lua_content = r#"
local app = {}

function app.on_start(session)
    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0)
    term.print("Welcome " .. session.callsign() .. "!\n")
    term.define_form(1)
    term.add_input_field("name", 2, 4, 15, "Bob")
    term.add_submit_button("ok", 2, 6)
    term.flush_form()

    session.await_input(1, function(sub)
        if type(sub) == "table" and sub.submit == "ok" then
            term.move_to(2, 8)
            term.print("Submitted: " .. (sub.name or ""))
        end
    end)
end

return app
"#;
        fs::write(temp_dir.join("main.lua"), lua_content).unwrap();

        let config = RunnerConfig {
            app_dir: temp_dir.clone(),
            user_nickname: "Alice".to_string(),
            ..Default::default()
        };

        let runner = AppRunner::new(config).unwrap();
        let lua = runner.setup_lua().unwrap();
        runner.run_start(&lua).unwrap();

        let screen_text = runner.screen.lock().unwrap().render_to_plain_text();
        assert!(screen_text.contains("Welcome Alice!"));

        // Test form submission
        let submission = serde_json::json!({
            "name": "SuperAlice",
            "submit": "ok"
        });
        runner.submit_input(&lua, submission).unwrap();

        let screen_after = runner.screen.lock().unwrap().render_to_plain_text();
        assert!(screen_after.contains("Submitted: SuperAlice"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_app_runner_db_and_session_apis() {
        let temp_dir = std::env::temp_dir().join(format!("bifrost_runner_db_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        let manifest_content = r#"
[app]
id = "db_test_app"
name = "DB Test App"
version = "1.0.0"
entry_point = "main.lua"

[[assets]]
name = "test_banner"
path = "assets/banner.ans"

[[assets]]
name = "test_menu"
path = "assets/menu.csv"
"#;
        fs::write(temp_dir.join("manifest.toml"), manifest_content).unwrap();

        let assets_dir = temp_dir.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("banner.ans"), "=== STARTER BANNER ===").unwrap();
        fs::write(assets_dir.join("menu.csv"), "# form_id=10\nopt1,opt1,PlayGame,2,10,P\nopt2,opt2,Scores,20,10,S").unwrap();

        let lua_content = r#"
local app = {}

function app.on_start(session)
    term.clear()
    term.render_asset("test_banner")
    term.render_menu("test_menu")

    db.set("scores", "alice", { points = 100 })
    local alice = db.get("scores", "alice")

    term.move_to(2, 14)
    if alice and alice.points == 100 then
        term.print("Points: " .. alice.points .. "\n")
    end

    if session.has_permission("admin") then
        term.print("Admin: YES\n")
    end

    if session.is_network_enabled() then
        term.print("Net: ENABLED\n")
    end
end

return app
"#;
        fs::write(temp_dir.join("main.lua"), lua_content).unwrap();

        let config = RunnerConfig {
            app_dir: temp_dir.clone(),
            user_nickname: "DevAdmin".to_string(),
            is_admin: true,
            ..Default::default()
        };

        let runner = AppRunner::new(config).unwrap();
        let lua = runner.setup_lua().unwrap();
        runner.run_start(&lua).unwrap();

        let screen_text = runner.screen.lock().unwrap().render_to_plain_text();
        assert!(screen_text.contains("=== STARTER BANNER ==="));
        assert!(screen_text.contains("[PlayGame]"));
        assert!(screen_text.contains("[Scores]"));
        assert!(screen_text.contains("Points: 100"));
        assert!(screen_text.contains("Admin: YES"));
        assert!(screen_text.contains("Net: ENABLED"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}

