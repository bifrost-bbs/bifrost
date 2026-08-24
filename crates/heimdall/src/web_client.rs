//! Virtual Web-based ANSI/CP437 BBS Test Client bridge over WebSocket.

use axum::extract::ws::{Message, WebSocket};
use bifrost_transport::{
    MeshBbsMessage, MockSocketTransport, RadioPacket, RadioTransport, SessionPayloadCache,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use crate::find_workspace_root;
use crate::logs::LogBuffer;

/// IBM PC CP437 to Unicode Character Mapping Table
pub const CP437_MAP: [char; 256] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂',
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╙', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ',
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormFieldClient {
    pub id: String,
    pub col: u8,
    pub row: u8,
    pub width: u8,
    pub height: u8,
    pub val: String,
    pub is_submit: bool,
    pub key: Option<char>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormStateClient {
    pub active: bool,
    pub form_id: u8,
    pub fields: Vec<FormFieldClient>,
    pub active_idx: usize,
    pub field_fg: u8,
    pub field_bg: u8,
    pub submit_fg: u8,
    pub submit_bg: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WsServerMsg {
    ScreenUpdate {
        lines: Vec<String>,
        raw_text: String,
        cursor_col: usize,
        cursor_row: usize,
        active_app: Option<String>,
    },
    Connected {
        node_id: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum WsClientMsg {
    #[serde(rename = "key")]
    Key { key: String },
    #[serde(rename = "reset")]
    Reset,
}

pub enum KeyAction {
    HandledLocally,
    SendBytes(Vec<u8>),
}

pub struct VirtualTerminalCanvas {
    pub width: usize,
    pub height: usize,
    pub grid: Vec<Vec<(char, u8)>>, // char, color_attr (high nibble BG, low nibble FG)
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub current_fg: u8,
    pub current_bg: u8,
    pub active_form: Option<FormStateClient>,
    pub log_buffer: Option<Arc<LogBuffer>>,
    pub dict: bifrost_compression::CompressionDictionary,
}

impl VirtualTerminalCanvas {
    pub fn new(width: usize, height: usize, log_buffer: Option<Arc<LogBuffer>>) -> Self {
        let grid = vec![vec![(' ', 0x07); width]; height];
        Self {
            width,
            height,
            grid,
            cursor_col: 0,
            cursor_row: 0,
            current_fg: 7, // Light Grey
            current_bg: 0, // Black
            active_form: None,
            log_buffer,
            dict: load_active_client_dictionary(),
        }
    }

    pub fn clear(&mut self) {
        let default_attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
        for row in self.grid.iter_mut() {
            for cell in row.iter_mut() {
                *cell = (' ', default_attr);
            }
        }
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.active_form = None;
    }

    pub fn put_char(&mut self, ch: char, attr: u8) {
        if self.cursor_row < self.height && self.cursor_col < self.width {
            self.grid[self.cursor_row][self.cursor_col] = (ch, attr);
            self.cursor_col += 1;
            if self.cursor_col >= self.width {
                self.cursor_col = 0;
                if self.cursor_row + 1 < self.height {
                    self.cursor_row += 1;
                }
            }
        }
    }

    pub fn apply_ansi_str(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if ch == '\x1b' && i + 1 < chars.len() && chars[i + 1] == '[' {
                // Parse ANSI Escape Sequence \x1b[ ... m or \x1b[ ... H
                let mut end = i + 2;
                while end < chars.len() && !chars[end].is_ascii_alphabetic() {
                    end += 1;
                }
                if end < chars.len() {
                    let cmd = chars[end];
                    let seq_str: String = chars[i + 2..end].iter().collect();
                    match cmd {
                        'm' => {
                            if seq_str.is_empty() || seq_str == "0" {
                                self.current_fg = 7;
                                self.current_bg = 0;
                            } else {
                                for part in seq_str.split(';') {
                                    if let Ok(code) = part.parse::<u8>() {
                                        match code {
                                            0 => { self.current_fg = 7; self.current_bg = 0; }
                                            1 => { self.current_fg |= 0x08; } // Bold / Bright
                                            30..=37 => { self.current_fg = (self.current_fg & 0x08) | (code - 30); }
                                            40..=47 => { self.current_bg = code - 40; }
                                            90..=97 => { self.current_fg = 8 + (code - 90); }
                                            100..=107 => { self.current_bg = 8 + (code - 100); }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        'H' | 'f' => {
                            let parts: Vec<&str> = seq_str.split(';').collect();
                            let row = parts.first().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).saturating_sub(1);
                            let col = parts.get(1).and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).saturating_sub(1);
                            self.cursor_row = row.min(self.height.saturating_sub(1));
                            self.cursor_col = col.min(self.width.saturating_sub(1));
                        }
                        'J' => {
                            self.clear();
                        }
                        _ => {}
                    }
                    i = end + 1;
                    continue;
                }
            }

            if ch == '\r' {
                self.cursor_col = 0;
            } else if ch == '\n' {
                self.cursor_col = 0;
                if self.cursor_row + 1 < self.height {
                    self.cursor_row += 1;
                }
            } else {
                let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                self.put_char(ch, attr);
            }
            i += 1;
        }
    }

    pub fn render_form_fields(&mut self) {
        if let Some(ref form) = self.active_form {
            if !form.active {
                return;
            }

            for (idx, field) in form.fields.iter().enumerate() {
                let is_active = idx == form.active_idx;
                if field.is_submit {
                    // Active submit button: swaps FG and BG
                    let (fg, bg) = if is_active {
                        (form.submit_bg, form.submit_fg)
                    } else {
                        (form.submit_fg, form.submit_bg)
                    };
                    let attr = (bg << 4) | (fg & 0x0F);
                    let label_text = if !field.val.is_empty() { &field.val } else { &field.id };
                    let label = format!("[ {} ]", label_text);
                    let r = field.row as usize;
                    let c = field.col as usize;
                    for (w, ch) in label.chars().enumerate() {
                        if r < self.height && c + w < self.width {
                            self.grid[r][c + w] = (ch, attr);
                        }
                    }
                } else {
                    // Active input field: swaps FG and BG
                    let (fg, bg) = if is_active {
                        (form.field_bg, form.field_fg)
                    } else {
                        (form.field_fg, form.field_bg)
                    };
                    let attr = (bg << 4) | (fg & 0x0F);
                    let val_chars: Vec<char> = field.val.chars().collect();

                    for r_off in 0..field.height as usize {
                        for w in 0..field.width as usize {
                            let curr_r = field.row as usize + r_off;
                            let curr_c = field.col as usize + w;
                            let char_idx = r_off * (field.width as usize) + w;
                            let ch = if char_idx < val_chars.len() { val_chars[char_idx] } else { ' ' };
                            if curr_r < self.height && curr_c < self.width {
                                self.grid[curr_r][curr_c] = (ch, attr);
                            }
                        }
                    }
                }
            }

            // Reposition terminal cursor at the focused field
            if !form.fields.is_empty() && form.active_idx < form.fields.len() {
                let active_field = &form.fields[form.active_idx];
                if active_field.is_submit {
                    self.cursor_row = (active_field.row as usize).min(self.height - 1);
                    self.cursor_col = (active_field.col as usize + 2).min(self.width - 1);
                } else {
                    let char_count = active_field.val.chars().count();
                    let r_off = char_count / active_field.width as usize;
                    let c_off = char_count % active_field.width as usize;
                    self.cursor_row = (active_field.row as usize + r_off).min(self.height - 1);
                    self.cursor_col = (active_field.col as usize + c_off).min(self.width - 1);
                }
            }
        }
    }

    pub fn process_key(&mut self, key: &str) -> KeyAction {
        if let Some(ref mut form) = self.active_form {
            if form.active && !form.fields.is_empty() {
                match key {
                    "Tab" | "ArrowDown" | "ArrowRight" | "Down" | "Right" => {
                        form.active_idx = (form.active_idx + 1) % form.fields.len();
                        self.render_form_fields();
                        return KeyAction::HandledLocally;
                    }
                    "ArrowUp" | "ArrowLeft" | "Up" | "Left" => {
                        form.active_idx = (form.active_idx + form.fields.len() - 1) % form.fields.len();
                        self.render_form_fields();
                        return KeyAction::HandledLocally;
                    }
                    "Backspace" => {
                        let idx = form.active_idx;
                        let field = &mut form.fields[idx];
                        if !field.is_submit && !field.val.is_empty() {
                            field.val.pop();
                            self.render_form_fields();
                        }
                        return KeyAction::HandledLocally;
                    }
                    "Enter" => {
                        let idx = form.active_idx;
                        let is_submit = form.fields[idx].is_submit;
                        if is_submit {
                            let submit_id = form.fields[idx].id.clone();
                            let mut map = HashMap::new();
                            for f in &form.fields {
                                if !f.is_submit {
                                    map.insert(f.id.clone(), f.val.clone());
                                }
                            }
                            map.insert("submit".to_string(), submit_id);
                            form.active = false;
                            form.fields.clear();
                            let json = serde_json::to_string(&map).unwrap_or_default();
                            return KeyAction::SendBytes(json.into_bytes());
                        } else {
                            // Advance focus to next field
                            form.active_idx = (form.active_idx + 1) % form.fields.len();
                            self.render_form_fields();
                            return KeyAction::HandledLocally;
                        }
                    }
                    "Escape" => {
                        return KeyAction::SendBytes(vec![0x1B]);
                    }
                    c_str if c_str.chars().count() == 1 => {
                        let ch = c_str.chars().next().unwrap();
                        if !ch.is_control() {
                            let idx = form.active_idx;
                            let current_is_submit = form.fields[idx].is_submit;
                            if !current_is_submit {
                                let field = &mut form.fields[idx];
                                let max_len = field.width as usize * field.height as usize;
                                if field.val.chars().count() < max_len {
                                    field.val.push(ch);
                                    self.render_form_fields();
                                }
                                return KeyAction::HandledLocally;
                            } else {
                                // In submit mode, check hotkeys
                                let lower_c = ch.to_ascii_lowercase();
                                let mut matched_idx = None;
                                for (i, f) in form.fields.iter().enumerate() {
                                    if f.is_submit {
                                        if let Some(k) = f.key {
                                            if k.to_ascii_lowercase() == lower_c {
                                                matched_idx = Some(i);
                                                break;
                                            }
                                        }
                                        let label_first = f.val.chars().next().map(|c| c.to_ascii_lowercase());
                                        let id_first = f.id.chars().next().map(|c| c.to_ascii_lowercase());
                                        if label_first == Some(lower_c) || id_first == Some(lower_c) {
                                            matched_idx = Some(i);
                                            break;
                                        }
                                    }
                                }

                                if let Some(target_idx) = matched_idx {
                                    let submit_id = form.fields[target_idx].id.clone();
                                    let mut map = HashMap::new();
                                    for f in &form.fields {
                                        if !f.is_submit {
                                            map.insert(f.id.clone(), f.val.clone());
                                        }
                                    }
                                    map.insert("submit".to_string(), submit_id);
                                    form.active = false;
                                    form.fields.clear();
                                    let json = serde_json::to_string(&map).unwrap_or_default();
                                    return KeyAction::SendBytes(json.into_bytes());
                                } else {
                                    return KeyAction::HandledLocally;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Non-form mode: transmit keys directly
        match key {
            "Enter" => KeyAction::SendBytes(vec![b'\n']),
            "Escape" => KeyAction::SendBytes(vec![0x1B]),
            "Backspace" => KeyAction::SendBytes(vec![0x08]),
            "Tab" => KeyAction::SendBytes(vec![b'\t']),
            "ArrowUp" | "Up" => KeyAction::SendBytes(vec![0x1B, 0x5B, 0x41]),
            "ArrowDown" | "Down" => KeyAction::SendBytes(vec![0x1B, 0x5B, 0x42]),
            "ArrowRight" | "Right" => KeyAction::SendBytes(vec![0x1B, 0x5B, 0x43]),
            "ArrowLeft" | "Left" => KeyAction::SendBytes(vec![0x1B, 0x5B, 0x44]),
            other => {
                if other.chars().count() == 1 {
                    KeyAction::SendBytes(other.as_bytes().to_vec())
                } else {
                    KeyAction::HandledLocally
                }
            }
        }
    }

    pub fn apply_bytecode(&mut self, bytecode: &[u8]) {
        let mut idx = 0;
        while idx < bytecode.len() {
            let byte = bytecode[idx];
            idx += 1;

            match byte {
                0x00..=0x1F => {
                    // Safe handling of control codes: do NOT draw as CP437 glyphs
                    match byte {
                        0x01 => {
                            // OP_CLEAR_SCREEN
                            self.clear();
                            if let Some(ref lb) = self.log_buffer {
                                lb.push("web_client", "DEBUG", "Bytecode: OP_CLEAR_SCREEN");
                            }
                        }
                        0x02 | b'\n' => {
                            // OP_CRLF
                            self.cursor_col = 0;
                            if self.cursor_row + 1 < self.height {
                                self.cursor_row += 1;
                            }
                        }
                        b'\r' => {
                            self.cursor_col = 0;
                        }
                        0x04 => {
                            // EndOfFrame
                            self.render_form_fields();
                        }
                        _ => {}
                    }
                }
                0xC0 => {
                    // OP_SET_COLOR (attr)
                    if idx < bytecode.len() {
                        let attr = bytecode[idx];
                        idx += 1;
                        self.current_fg = attr & 0x0F;
                        self.current_bg = (attr >> 4) & 0x0F;
                    }
                }
                0xC1 => {
                    // OP_RLE_GLYPH (count, glyph_byte)
                    if idx + 1 < bytecode.len() {
                        let count = bytecode[idx] as usize;
                        let glyph_byte = bytecode[idx + 1];
                        idx += 2;
                        let glyph = CP437_MAP[glyph_byte as usize];
                        let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                        for _ in 0..count {
                            self.put_char(glyph, attr);
                        }
                    }
                }
                0xC2 => {
                    // OP_RLE_SPACE (count)
                    if idx < bytecode.len() {
                        let count = bytecode[idx] as usize;
                        idx += 1;
                        let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                        for _ in 0..count {
                            self.put_char(' ', attr);
                        }
                    }
                }
                0xC3 => {
                    // OP_CURSOR_ABS (col, row)
                    if idx + 1 < bytecode.len() {
                        let col = bytecode[idx] as usize;
                        let row = bytecode[idx + 1] as usize;
                        idx += 2;
                        self.cursor_col = col.min(self.width.saturating_sub(1));
                        self.cursor_row = row.min(self.height.saturating_sub(1));
                    }
                }
                0xC4 => {
                    // OP_CURSOR_REL (dcol, drow)
                    if idx + 1 < bytecode.len() {
                        let dcol = bytecode[idx] as i8;
                        let drow = bytecode[idx + 1] as i8;
                        idx += 2;
                        self.cursor_col = (self.cursor_col as isize + dcol as isize).clamp(0, self.width as isize - 1) as usize;
                        self.cursor_row = (self.cursor_row as isize + drow as isize).clamp(0, self.height as isize - 1) as usize;
                    }
                }
                0xC5 => {
                    // OP_RENDER_ASSET (asset_id u16)
                    if idx + 1 < bytecode.len() {
                        let asset_id = u16::from_be_bytes([bytecode[idx], bytecode[idx + 1]]);
                        idx += 2;
                        self.render_asset_by_id(asset_id);
                    }
                }
                0xC7 => {
                    // OP_RENDER_TEMPLATE (asset_id u16, param_count u8, [param_len u8, param_bytes]*)
                    if idx + 2 < bytecode.len() {
                        let asset_id = u16::from_be_bytes([bytecode[idx], bytecode[idx + 1]]);
                        let param_count = bytecode[idx + 2] as usize;
                        idx += 3;
                        let mut params = Vec::new();
                        for _ in 0..param_count {
                            if idx < bytecode.len() {
                                let p_len = bytecode[idx] as usize;
                                idx += 1;
                                if idx + p_len <= bytecode.len() {
                                    let s = String::from_utf8_lossy(&bytecode[idx..idx + p_len]).to_string();
                                    idx += p_len;
                                    params.push(s);
                                }
                            }
                        }
                        if let Some((template_str, desc)) = get_asset_content_by_id(asset_id) {
                            if let Some(ref lb) = self.log_buffer {
                                lb.push("web_client", "INFO", &format!("Rendering template ID 0x{:04X} -> '{}'", asset_id, desc));
                            }
                            let expanded = bifrost_bbs::substitute_template(&template_str, &params);
                            self.apply_ansi_str(&expanded);
                        } else if let Some(ref lb) = self.log_buffer {
                            lb.push("web_client", "WARN", &format!("Failed to resolve template ID 0x{:04X}", asset_id));
                        }
                    }
                }
                0xC8 => {
                    // OP_RENDER_MENU (asset_id u16, toggle_mask u32)
                    if idx + 5 < bytecode.len() {
                        let asset_id = u16::from_be_bytes([bytecode[idx], bytecode[idx + 1]]);
                        let mask = u32::from_be_bytes([bytecode[idx + 2], bytecode[idx + 3], bytecode[idx + 4], bytecode[idx + 5]]);
                        idx += 6;

                        if let Some((menu_csv, desc)) = get_asset_content_by_id(asset_id) {
                            if let Some(ref lb) = self.log_buffer {
                                lb.push("web_client", "INFO", &format!("Rendering menu ID 0x{:04X} -> '{}'", asset_id, desc));
                            }
                            let menu_def = bifrost_bbs::parse_menu_csv(&menu_csv);
                            let f_fg = menu_def.field_fg.unwrap_or(7);
                            let f_bg = menu_def.field_bg.unwrap_or(0);
                            let s_fg = menu_def.submit_fg.unwrap_or(0);
                            let s_bg = menu_def.submit_bg.unwrap_or(7);

                            let mut fields = Vec::new();
                            let align_mode = menu_def.align.as_deref().unwrap_or("top_left");
                            let is_bottom = align_mode.starts_with("bottom");
                            let is_center = align_mode.ends_with("center") || align_mode == "center";
                            let is_right = align_mode.ends_with("right") || align_mode == "right";

                            let max_col = if self.width > 10 { self.width as u8 - 2 } else { 78 };
                            let term_h = self.height as u8;

                            // Pre-filter enabled buttons and compute row totals for alignment
                            let mut enabled_buttons = Vec::new();
                            let mut row_widths: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();

                            for (b_idx, btn) in menu_def.buttons.iter().enumerate() {
                                if b_idx < 32 && (mask & (1 << b_idx)) != 0 {
                                    let btn_width = (btn.label.len() as u8) + 4;
                                    let base_row = if is_bottom {
                                        if btn.row > 0 && btn.row < 10 {
                                            term_h.saturating_sub(btn.row + 1)
                                        } else {
                                            term_h.saturating_sub(3)
                                        }
                                    } else if btn.row == 0 {
                                        12
                                    } else {
                                        btn.row
                                    };
                                    enabled_buttons.push((btn, btn_width, base_row));

                                    let entry = row_widths.entry(base_row).or_insert(0);
                                    if *entry > 0 { *entry += 1; }
                                    *entry += btn_width;
                                }
                            }

                            let mut row_cols: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();

                            for (btn, btn_width, base_row) in enabled_buttons {
                                let mut cur_row = base_row;
                                let default_start_col = if is_center {
                                    let tot = *row_widths.get(&base_row).unwrap_or(&btn_width);
                                    if max_col > tot { ((max_col + 2 - tot) / 2).max(2) } else { 2 }
                                } else if is_right {
                                    let tot = *row_widths.get(&base_row).unwrap_or(&btn_width);
                                    if max_col > tot { (max_col + 2 - tot).max(2) } else { 2 }
                                } else {
                                    2
                                };

                                let mut cur_col = *row_cols.get(&cur_row).unwrap_or(&default_start_col);

                                if cur_col + btn_width > max_col && cur_col > default_start_col {
                                    cur_row += 2;
                                    cur_col = *row_cols.get(&cur_row).unwrap_or(&default_start_col);
                                }

                                let field_col = cur_col;
                                let field_row = cur_row;
                                row_cols.insert(cur_row, cur_col + btn_width + 1); // 1 space spacing between buttons

                                fields.push(FormFieldClient {
                                    id: btn.id.clone(),
                                    col: field_col,
                                    row: field_row,
                                    width: btn_width,
                                    height: 1,
                                    val: btn.label.clone(),
                                    is_submit: true,
                                    key: btn.key,
                                });
                            }

                            self.active_form = Some(FormStateClient {
                                active: true,
                                form_id: menu_def.form_id,
                                fields,
                                active_idx: 0,
                                field_fg: f_fg,
                                field_bg: f_bg,
                                submit_fg: s_fg,
                                submit_bg: s_bg,
                            });
                        } else if let Some(ref lb) = self.log_buffer {
                            lb.push("web_client", "WARN", &format!("Failed to resolve menu ID 0x{:04X}", asset_id));
                        }
                    }
                }
                0xD0 => {
                    // OP_FORM_START (form_id, field_fg, field_bg, submit_fg, submit_bg)
                    if idx + 4 < bytecode.len() {
                        let form_id = bytecode[idx];
                        let field_fg = bytecode[idx + 1];
                        let field_bg = bytecode[idx + 2];
                        let submit_fg = bytecode[idx + 3];
                        let submit_bg = bytecode[idx + 4];
                        idx += 5;
                        self.active_form = Some(FormStateClient {
                            active: true,
                            form_id,
                            fields: Vec::new(),
                            active_idx: 0,
                            field_fg,
                            field_bg,
                            submit_fg,
                            submit_bg,
                        });
                        if let Some(ref lb) = self.log_buffer {
                            lb.push("web_client", "DEBUG", &format!("Bytecode: OP_FORM_START (form_id: {})", form_id));
                        }
                    }
                }
                0xD1 => {
                    // OP_FORM_FIELD (col, row, width, id_len, id_str, val_len, val_str)
                    if idx + 3 < bytecode.len() {
                        let col = bytecode[idx];
                        let row = bytecode[idx + 1];
                        let width = bytecode[idx + 2];
                        let id_len = bytecode[idx + 3] as usize;
                        idx += 4;
                        if idx + id_len <= bytecode.len() {
                            let id_str = String::from_utf8_lossy(&bytecode[idx..idx + id_len]).to_string();
                            idx += id_len;
                            if idx < bytecode.len() {
                                let val_len = bytecode[idx] as usize;
                                idx += 1;
                                let val_str = if idx + val_len <= bytecode.len() {
                                    let s = String::from_utf8_lossy(&bytecode[idx..idx + val_len]).to_string();
                                    idx += val_len;
                                    s
                                } else {
                                    String::new()
                                };

                                if let Some(ref mut f) = self.active_form {
                                    f.fields.push(FormFieldClient {
                                        id: id_str,
                                        col,
                                        row,
                                        width,
                                        height: 1,
                                        val: val_str,
                                        is_submit: false,
                                        key: None,
                                    });
                                }
                            }
                        }
                    }
                }
                0xD2 => {
                    // OP_FORM_SUBMIT (col, row, id_len, id_str)
                    if idx + 2 < bytecode.len() {
                        let col = bytecode[idx];
                        let row = bytecode[idx + 1];
                        let id_len = bytecode[idx + 2] as usize;
                        idx += 3;
                        if idx + id_len <= bytecode.len() {
                            let id_str = String::from_utf8_lossy(&bytecode[idx..idx + id_len]).to_string();
                            idx += id_len;

                            if let Some(ref mut f) = self.active_form {
                                f.fields.push(FormFieldClient {
                                    id: id_str.clone(),
                                    col,
                                    row,
                                    width: (id_len as u8) + 4,
                                    height: 1,
                                    val: String::new(),
                                    is_submit: true,
                                    key: None,
                                });
                            }
                            if let Some(ref lb) = self.log_buffer {
                                lb.push("web_client", "DEBUG", &format!("Bytecode: OP_FORM_SUBMIT ('{}')", id_str));
                            }
                        }
                    }
                }
                0xD3 => {
                    // OP_FORM_END: Render all form fields with active focus styling
                    self.render_form_fields();
                    if let Some(ref lb) = self.log_buffer {
                        lb.push("web_client", "DEBUG", "Bytecode: OP_FORM_END");
                    }
                }
                0xD4 => {
                    // OP_FORM_FIELD_MULTILINE (col, row, width, height, id_len, id_str, val_len, val_str)
                    if idx + 4 < bytecode.len() {
                        let col = bytecode[idx];
                        let row = bytecode[idx + 1];
                        let width = bytecode[idx + 2];
                        let height = bytecode[idx + 3];
                        let id_len = bytecode[idx + 4] as usize;
                        idx += 5;
                        if idx + id_len <= bytecode.len() {
                            let id_str = String::from_utf8_lossy(&bytecode[idx..idx + id_len]).to_string();
                            idx += id_len;
                            if idx < bytecode.len() {
                                let val_len = bytecode[idx] as usize;
                                idx += 1;
                                let val_str = if idx + val_len <= bytecode.len() {
                                    let s = String::from_utf8_lossy(&bytecode[idx..idx + val_len]).to_string();
                                    idx += val_len;
                                    s
                                } else {
                                    String::new()
                                };

                                if let Some(ref mut f) = self.active_form {
                                    f.fields.push(FormFieldClient {
                                        id: id_str,
                                        col,
                                        row,
                                        width,
                                        height,
                                        val: val_str,
                                        is_submit: false,
                                        key: None,
                                    });
                                }
                            }
                        }
                    }
                }
                0xFD => {
                    // OP_DICT_TOKEN (token_id)
                    if idx < bytecode.len() {
                        let token_id = bytecode[idx] as usize;
                        idx += 1;
                        if let Some(tok_bytes) = self.dict.tokens().get(token_id).cloned() {
                            for b in tok_bytes {
                                if b == b'\r' {
                                    self.cursor_col = 0;
                                } else if b == b'\n' {
                                    self.cursor_col = 0;
                                    if self.cursor_row + 1 < self.height {
                                        self.cursor_row += 1;
                                    }
                                } else {
                                    let ch = CP437_MAP[b as usize];
                                    let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                                    self.put_char(ch, attr);
                                }
                            }
                        }
                    }
                }
                0xFE => {
                    // OP_RAW_CP437 (len, bytes...)
                    if idx < bytecode.len() {
                        let len = bytecode[idx] as usize;
                        idx += 1;
                        let end = (idx + len).min(bytecode.len());
                        let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                        for &b in &bytecode[idx..end] {
                            self.put_char(CP437_MAP[b as usize], attr);
                        }
                        idx = end;
                    }
                }
                _ => {
                    // Literal CP437 character (>= 0x20)
                    let ch = CP437_MAP[byte as usize];
                    let attr = (self.current_bg << 4) | (self.current_fg & 0x0F);
                    self.put_char(ch, attr);
                }
            }
        }
    }

    fn render_asset_by_id(&mut self, asset_id: u16) {
        if let Some((content, desc)) = get_asset_content_by_id(asset_id) {
            if let Some(ref lb) = self.log_buffer {
                lb.push("web_client", "INFO", &format!("Rendering asset ID 0x{:04X} -> '{}'", asset_id, desc));
            }
            self.apply_ansi_str(&content);
        } else if let Some(ref lb) = self.log_buffer {
            lb.push("web_client", "WARN", &format!("Failed to resolve asset ID 0x{:04X}", asset_id));
        }
    }

    pub fn to_html_lines(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.height);
        
        // Standard 16-color ANSI Palette (0..15)
        let palette = [
            "#000000", // 0: Black
            "#aa0000", // 1: Red
            "#00aa00", // 2: Green
            "#aa5500", // 3: Yellow / Brown
            "#0000aa", // 4: Blue
            "#aa00aa", // 5: Magenta
            "#00aaaa", // 6: Cyan
            "#aaaaaa", // 7: Light Grey
            "#555555", // 8: Dark Grey
            "#ff5555", // 9: Bright Red
            "#55ff55", // 10: Bright Green
            "#ffff55", // 11: Bright Yellow
            "#5555ff", // 12: Bright Blue
            "#ff55ff", // 13: Bright Magenta
            "#55ffff", // 14: Bright Cyan
            "#ffffff", // 15: Bright White
        ];

        for (row_idx, row) in self.grid.iter().enumerate() {
            let mut line_html = String::new();
            let mut last_fg = 99;
            let mut last_bg = 99;
            let mut span_open = false;

            for (col_idx, &(ch, attr)) in row.iter().enumerate() {
                let is_cursor = row_idx == self.cursor_row && col_idx == self.cursor_col;
                let fg = attr & 0x0F;
                let bg = (attr >> 4) & 0x0F;

                if fg != last_fg || bg != last_bg || is_cursor {
                    if span_open {
                        line_html.push_str("</span>");
                    }
                    let fg_color = palette.get(fg as usize).unwrap_or(&"#aaaaaa");
                    let bg_color = palette.get(bg as usize).unwrap_or(&"#000000");
                    let cursor_class = if is_cursor { " term-cursor" } else { "" };
                    line_html.push_str(&format!(
                        r#"<span class="c-cell{}" style="color:{};background-color:{};">"#,
                        cursor_class, fg_color, bg_color
                    ));
                    last_fg = fg;
                    last_bg = bg;
                    span_open = true;
                }

                match ch {
                    '&' => line_html.push_str("&amp;"),
                    '<' => line_html.push_str("&lt;"),
                    '>' => line_html.push_str("&gt;"),
                    '"' => line_html.push_str("&quot;"),
                    ' ' => line_html.push(' '),
                    _ => line_html.push(ch),
                }

                if is_cursor && span_open {
                    line_html.push_str("</span>");
                    span_open = false;
                    last_fg = 99;
                    last_bg = 99;
                }
            }

            if span_open {
                line_html.push_str("</span>");
            }
            lines.push(line_html);
        }

        lines
    }

    pub fn to_raw_text(&self) -> String {
        let mut text = String::new();
        for row in &self.grid {
            for &(ch, _) in row {
                text.push(ch);
            }
            text.push('\n');
        }
        text
    }
}

pub fn get_asset_content_by_id(asset_id: u16) -> Option<(String, String)> {
    let root = find_workspace_root();

    // 1. Check .client_cache/<id>.ans
    let hex_name = format!("{:04x}.ans", asset_id);
    let cache_candidates = [
        root.join(".client_cache").join(&hex_name),
        PathBuf::from(".client_cache").join(&hex_name),
        PathBuf::from("../../.client_cache").join(&hex_name),
    ];
    for cc in cache_candidates {
        if let Ok(content) = std::fs::read_to_string(&cc) {
            return Some((content, cc.to_string_lossy().to_string()));
        }
    }

    // 2. Canonical dynamic asset registry matching BBS server allocation exactly
    let enabled_apps = vec![
        "messages".to_string(),
        "profile".to_string(),
        "minidungeon".to_string(),
        "admin".to_string(),
        "marketplace".to_string(),
        "weather".to_string(),
        "voidtrader".to_string(),
    ];
    let manifest_map = bifrost_bbs::load_app_manifests(&enabled_apps);
    if let Some((name, rel_path)) = manifest_map.get(&asset_id) {
        let candidates = [
            root.join(rel_path),
            PathBuf::from(rel_path),
            PathBuf::from("../../").join(rel_path),
        ];
        for cand in candidates {
            if let Ok(ans_str) = std::fs::read_to_string(&cand) {
                return Some((ans_str, format!("{} [{}]", cand.to_string_lossy(), name)));
            }
        }
    }

    None
}

pub fn load_active_client_dictionary() -> bifrost_compression::CompressionDictionary {
    let root = find_workspace_root();
    let candidates = [
        root.join("config/bbs_dict.bin"),
        PathBuf::from("config/bbs_dict.bin"),
        PathBuf::from("../../config/bbs_dict.bin"),
        root.join(".client_cache/dict.bin"),
        PathBuf::from(".client_cache/dict.bin"),
    ];
    for p in candidates {
        if let Ok(bytes) = std::fs::read(&p) {
            if let Ok(dict) = bifrost_compression::CompressionDictionary::from_bytes(&bytes) {
                log::info!(
                    "Web client loaded domain dictionary from {:?} ({} tokens, CRC: 0x{:08X})",
                    p,
                    dict.tokens().len(),
                    dict.crc32()
                );
                return dict;
            }
        }
    }
    log::info!("Web client using standard static dictionary fallback");
    bifrost_compression::CompressionDictionary::standard_static()
}

pub async fn handle_web_terminal_ws(
    socket: WebSocket,
    radio_port: String,
    log_buf: Arc<LogBuffer>,
    authenticated_node: Option<[u8; 32]>,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create client radio transport connected to the BBS mock server
    let target_addr = radio_port;
    let client_transport = Arc::new(MockSocketTransport::new_client(target_addr, 0.0, 0, 200));

    // Use authenticated persistent node identity if provided, otherwise generate random
    let client_node = authenticated_node.unwrap_or_else(|| {
        let mut node = [0u8; 32];
        for b in node.iter_mut() {
            *b = rand_byte();
        }
        node
    });
    let node_hex = hex_encode(&client_node);
    let short_id = format!("{:02x}{:02x}..{:02x}{:02x}", client_node[0], client_node[1], client_node[30], client_node[31]);

    log_buf.push("web_client", "INFO", &format!("Web terminal client connected (Node ID: {})", short_id));

    let _ = ws_sender
        .send(Message::Text(
            serde_json::to_string(&WsServerMsg::Connected {
                node_id: node_hex.clone(),
            })
            .unwrap(),
        ))
        .await;

    // Send Handshake packet to BBS (Channel 0x03, Opcode 0x01)
    let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
    if let Ok(fragments) = handshake_msg.to_fragments(200) {
        for frag in fragments {
            let packet = RadioPacket {
                is_broadcast: false,
                src_node: client_node,
                dst_node: [0; 32],
                payload: frag,
                signal_rssi: -50,
                signal_snr: 10,
            };
            let _ = client_transport.send_packet(packet).await;
        }
    }

    let (ws_out_tx, mut ws_out_rx) = mpsc::channel::<WsServerMsg>(100);

    // Forward outbound msgs to WebSocket
    let ws_send_task = tokio::spawn(async move {
        while let Some(msg) = ws_out_rx.recv().await {
            if let Ok(json_str) = serde_json::to_string(&msg) {
                if ws_sender.send(Message::Text(json_str)).await.is_err() {
                    break;
                }
            }
        }
    });

    let canvas_lock = Arc::new(tokio::sync::Mutex::new(VirtualTerminalCanvas::new(80, 25, Some(log_buf.clone()))));
    let client_dict = load_active_client_dictionary();
    let client_session_cache = Arc::new(tokio::sync::Mutex::new(SessionPayloadCache::new(50)));

    // Task to listen for radio packets from BBS
    let rx_transport = client_transport.clone();
    let rx_canvas = canvas_lock.clone();
    let rx_ws_tx = ws_out_tx.clone();
    let rx_session_cache = client_session_cache.clone();
    let rx_log_buf = log_buf.clone();

    let radio_rx_task = tokio::spawn(async move {
        let mut assembler = bifrost_transport::MessageReassembler::new();

        loop {
            match rx_transport.receive_packet().await {
                Ok(packet) => {
                    if packet.is_broadcast {
                        continue;
                    }
                    // Filter packets strictly meant for this web client session
                    if packet.dst_node != [0; 32] && packet.dst_node != client_node {
                        continue;
                    }

                    if let Ok(Some(msg)) = assembler.process_packet(packet.src_node, &packet.payload) {
                        let payload = if (msg.flags & 0x08) != 0 {
                            // Hash-referencing previous session payload
                            if msg.payload.len() >= 4 {
                                let crc = u32::from_be_bytes([
                                    msg.payload[0],
                                    msg.payload[1],
                                    msg.payload[2],
                                    msg.payload[3],
                                ]);
                                let sc = rx_session_cache.lock().await;
                                if let Some(cached) = sc.get(crc) {
                                    rx_log_buf.push("web_client", "DEBUG", &format!("[SESSION DEDUP] Hit for template 0x{:08X} ({} B)", crc, cached.len()));
                                    cached.clone()
                                } else {
                                    rx_log_buf.push("web_client", "WARN", &format!("[SESSION DEDUP] Miss for template 0x{:08X}", crc));
                                    msg.payload
                                }
                            } else {
                                msg.payload
                            }
                        } else if (msg.flags & 0x06) != 0 {
                            match bifrost_ansi::decompress_bytecode_adaptive(
                                msg.flags,
                                &msg.payload,
                                Some(&client_dict),
                            ) {
                                Ok(decomp) => {
                                    let crc = bifrost_transport::crc32(&decomp);
                                    let mut sc = rx_session_cache.lock().await;
                                    sc.insert(crc, decomp.clone());
                                    decomp
                                }
                                Err(e) => {
                                    rx_log_buf.push("web_client", "ERROR", &format!("Bytecode decompress error: {:?}", e));
                                    msg.payload
                                }
                            }
                        } else {
                            let crc = bifrost_transport::crc32(&msg.payload);
                            let mut sc = rx_session_cache.lock().await;
                            sc.insert(crc, msg.payload.clone());
                            msg.payload
                        };

                        rx_log_buf.push("web_client", "DEBUG", &format!("[RADIO RX] Decoded frame: {} B (flags: 0x{:02X}, opcode: 0x{:02X})", payload.len(), msg.flags, msg.opcode));

                        // Apply to shadow canvas
                        let mut canvas = rx_canvas.lock().await;
                        canvas.apply_bytecode(&payload);

                        let lines = canvas.to_html_lines();
                        let raw_text = canvas.to_raw_text();
                        let c_col = canvas.cursor_col;
                        let c_row = canvas.cursor_row;

                        let _ = rx_ws_tx
                            .send(WsServerMsg::ScreenUpdate {
                                lines,
                                raw_text,
                                cursor_col: c_col,
                                cursor_row: c_row,
                                active_app: None,
                            })
                            .await;
                    }
                }
                Err(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
            }
        }
    });

    // Task to listen for browser WebSocket input events and send to BBS
    let tx_transport = client_transport.clone();
    let tx_canvas = canvas_lock.clone();
    let tx_ws_tx = ws_out_tx.clone();
    let tx_log_buf = log_buf.clone();

    while let Some(msg_res) = ws_receiver.next().await {
        if let Ok(Message::Text(text)) = msg_res {
            if let Ok(client_msg) = serde_json::from_str::<WsClientMsg>(&text) {
                match client_msg {
                    WsClientMsg::Key { key } => {
                        let action = {
                            let mut canvas = tx_canvas.lock().await;
                            canvas.process_key(&key)
                        };

                        match action {
                            KeyAction::HandledLocally => {
                                tx_log_buf.push("web_client", "DEBUG", &format!("Key '{}' handled locally (form navigation / text update)", key));
                                // Form focus / text updated locally in canvas, update browser screen!
                                let canvas = tx_canvas.lock().await;
                                let lines = canvas.to_html_lines();
                                let raw_text = canvas.to_raw_text();
                                let c_col = canvas.cursor_col;
                                let c_row = canvas.cursor_row;
                                let _ = tx_ws_tx
                                    .send(WsServerMsg::ScreenUpdate {
                                        lines,
                                        raw_text,
                                        cursor_col: c_col,
                                        cursor_row: c_row,
                                        active_app: None,
                                    })
                                    .await;
                            }
                            KeyAction::SendBytes(payload_bytes) => {
                                tx_log_buf.push("web_client", "INFO", &format!("Key '{}' dispatched packet ({} B) to BBS", key, payload_bytes.len()));
                                let bbs_msg = MeshBbsMessage::new(0x02, 0x02, 0x00, payload_bytes);
                                if let Ok(frags) = bbs_msg.to_fragments(200) {
                                    for frag in frags {
                                        let pkt = RadioPacket {
                                            is_broadcast: false,
                                            src_node: client_node,
                                            dst_node: [0; 32],
                                            payload: frag,
                                            signal_rssi: -50,
                                            signal_snr: 10,
                                        };
                                        let _ = tx_transport.send_packet(pkt).await;
                                    }
                                }
                            }
                        }
                    }
                    WsClientMsg::Reset => {
                        tx_log_buf.push("web_client", "INFO", "Terminal canvas reset");
                        let mut canvas = tx_canvas.lock().await;
                        canvas.clear();
                    }
                }
            }
        }
    }

    log_buf.push("web_client", "INFO", &format!("Web terminal client disconnected (Node ID: {})", short_id));
    radio_rx_task.abort();
    ws_send_task.abort();
}

fn rand_byte() -> u8 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        & 0xFF) as u8
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_ansi::Opcode;

    #[test]
    fn test_virtual_terminal_canvas_basic() {
        let mut canvas = VirtualTerminalCanvas::new(80, 25, None);
        assert_eq!(canvas.width, 80);
        assert_eq!(canvas.height, 25);

        let mut bytecode = Vec::new();
        bytecode.push(Opcode::ClearScreen as u8);
        bytecode.push(Opcode::SetColor as u8);
        bytecode.push(0x0E); // Cyan on Black
        bytecode.extend_from_slice(b"HELLO BIFROST");

        canvas.apply_bytecode(&bytecode);
        assert_eq!(canvas.grid[0][0], ('H', 0x0E));
        assert_eq!(canvas.grid[0][1], ('E', 0x0E));

        let html = canvas.to_html_lines();
        assert_eq!(html.len(), 25);
        assert!(html[0].contains("HELLO BIFROST"));
    }

    #[test]
    fn test_virtual_terminal_form_navigation_and_input() {
        let mut canvas = VirtualTerminalCanvas::new(80, 25, None);
        let mut bytecode = Vec::new();
        bytecode.push(Opcode::ClearScreen as u8);
        bytecode.push(0xD0); // FormStart
        bytecode.extend_from_slice(&[1, 0, 14, 14, 4]); // form 1, field fg=0, bg=14, submit fg=14, bg=4
        bytecode.push(0xD1); // FormField
        bytecode.extend_from_slice(&[5, 2, 10, 4]); // col 5, row 2, width 10, id_len 4
        bytecode.extend_from_slice(b"nick");
        bytecode.push(4); // val_len 4
        bytecode.extend_from_slice(b"User");
        bytecode.push(0xD2); // FormSubmit
        bytecode.extend_from_slice(&[5, 4, 8]); // col 5, row 4, id_len 8
        bytecode.extend_from_slice(b"register");
        bytecode.push(0xD3); // FormEnd

        canvas.apply_bytecode(&bytecode);
        assert!(canvas.active_form.is_some());
        
        // Typing into input field
        match canvas.process_key("1") {
            KeyAction::HandledLocally => {
                let form = canvas.active_form.as_ref().unwrap();
                assert_eq!(form.fields[0].val, "User1");
            }
            _ => panic!("Expected local handling for char input in active field"),
        }

        // Backspace
        match canvas.process_key("Backspace") {
            KeyAction::HandledLocally => {
                let form = canvas.active_form.as_ref().unwrap();
                assert_eq!(form.fields[0].val, "User");
            }
            _ => panic!("Expected local handling for backspace in active field"),
        }

        // Tab moves focus to submit button
        match canvas.process_key("Tab") {
            KeyAction::HandledLocally => {
                let form = canvas.active_form.as_ref().unwrap();
                assert_eq!(form.active_idx, 1);
            }
            _ => panic!("Expected local handling for tab navigation"),
        }

        // Enter on submit button sends JSON payload to BBS
        match canvas.process_key("Enter") {
            KeyAction::SendBytes(bytes) => {
                let json_str = String::from_utf8(bytes).unwrap();
                assert!(json_str.contains("\"submit\":\"register\""));
                assert!(json_str.contains("\"nick\":\"User\""));
            }
            _ => panic!("Expected SendBytes on submit button Enter"),
        }
    }

    #[test]
    fn test_virtual_terminal_ansi_string() {
        let mut canvas = VirtualTerminalCanvas::new(80, 25, None);
        canvas.apply_ansi_str("\x1b[2J\x1b[1;1H\x1b[32mGREEN TEXT\x1b[0m");
        assert_eq!(canvas.grid[0][0], ('G', 0x02));
        assert_eq!(canvas.grid[0][1], ('R', 0x02));
        let raw = canvas.to_raw_text();
        assert!(raw.contains("GREEN TEXT"));
    }

    #[test]
    fn test_virtual_terminal_rle_and_multiline() {
        let mut canvas = VirtualTerminalCanvas::new(80, 25, None);
        let mut bytecode = Vec::new();
        bytecode.push(0x01); // ClearScreen
        bytecode.push(0xC1); // RleGlyph
        bytecode.push(5); // count
        bytecode.push(0xDB); // CP437 full block
        bytecode.push(0xC2); // RleSpace
        bytecode.push(3); // count
        bytecode.push(0xC3); // CursorAbs
        bytecode.push(10);
        bytecode.push(5);
        bytecode.push(0xD0); // FormStart
        bytecode.extend_from_slice(&[1, 15, 4, 0, 7]);
        bytecode.push(0xD4); // FormFieldMultiline
        bytecode.extend_from_slice(&[10, 5, 8, 2, 4]); // col 10, row 5, w 8, h 2, id_len 4
        bytecode.extend_from_slice(b"desc");
        bytecode.push(4); // val_len 4
        bytecode.extend_from_slice(b"LINE");
        bytecode.push(0xD3); // FormEnd

        canvas.apply_bytecode(&bytecode);
        assert_eq!(canvas.grid[0][0], ('█', 0x07));
        assert_eq!(canvas.grid[0][4], ('█', 0x07));
        assert_eq!(canvas.grid[0][5], (' ', 0x07));
        assert!(canvas.active_form.is_some());
    }

    #[test]
    fn test_asset_resolution_by_id() {
        let main_banner = get_asset_content_by_id(0x0101);
        assert!(main_banner.is_some(), "Main menu banner 0x0101 should resolve");
        let (content1, desc1) = main_banner.unwrap();
        assert!(content1.contains("Bifrost") || content1.contains("___"), "Should contain Bifrost banner art");
        assert!(desc1.contains("main_menu_banner.ans"));

        // With new template and menu assets registered, find dungeon and voidtrader assets
        let dungeon_banner = get_asset_content_by_id(0x0104);
        assert!(dungeon_banner.is_some(), "Dungeon banner 0x0104 should resolve");
        let (content2, desc2) = dungeon_banner.unwrap();
        assert!(desc2.contains("dungeon_banner.ans"));
        assert_ne!(content1, content2, "Dungeon banner must be distinct from main menu banner");

        let vt_banner = get_asset_content_by_id(0x0108);
        assert!(vt_banner.is_some(), "Void Trader banner 0x0108 should resolve");
        let (content3, desc3) = vt_banner.unwrap();
        assert!(desc3.contains("voidtrader_banner.ans"));
        assert_ne!(content1, content3, "Void Trader banner must be distinct from main menu banner");
    }

    #[test]
    fn test_main_menu_canvas_rendering() {
        let mut canvas = VirtualTerminalCanvas::new(80, 25, None);
        let payload = [
            1, 197, 1, 1, 195, 2, 7, 192, 7, 72, 101, 108, 108, 111, 44, 32, 84, 101, 115, 116,
            67, 108, 105, 101, 110, 116, 33, 10, 10, 83, 101, 108, 101, 99, 116, 32, 111, 112,
            116, 105, 111, 110, 115, 32, 117, 115, 105, 110, 103, 32, 84, 97, 98, 47, 65, 114,
            114, 111, 119, 115, 32, 97, 110, 100, 32, 69, 110, 116, 101, 114, 58, 10, 10, 208,
            10, 15, 4, 0, 7, 200, 1, 3, 0, 0, 0, 255, 211, 4,
        ];
        canvas.apply_bytecode(&payload);
        println!("Canvas raw text:\n{}", canvas.to_raw_text());
        let html_lines = canvas.to_html_lines();
        for (idx, line) in html_lines.iter().enumerate() {
            println!("Line {:02}: {}", idx, line);
        }
        assert!(canvas.active_form.is_some(), "Form should be active");
        let form = canvas.active_form.as_ref().unwrap();
        assert_eq!(form.fields.len(), 8, "Should have 8 buttons");
        assert_eq!(form.fields[0].id, "read_boards");
        assert_eq!(form.fields[0].val, "MessageBoards");

        // Test hotkey submission with 'v' for Void Trader
        match canvas.process_key("v") {
            KeyAction::SendBytes(bytes) => {
                let json_str = String::from_utf8(bytes).unwrap();
                assert!(json_str.contains("\"submit\":\"voidtrader\""), "Should submit voidtrader on 'v' hotkey");
            }
            _ => panic!("Expected SendBytes on 'v' hotkey"),
        }
    }
}
