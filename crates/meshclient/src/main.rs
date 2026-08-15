//! Interactive client terminal emulator for MeshBBS testing over mock radio socket.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meshcore_transport::{
    MeshBbsMessage, MessageReassembler, MockSocketTransport, RadioPacket, RadioTransport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Full,    // 80x25
    Compact, // 40x25
}

impl LayoutMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Full => Self::Compact,
            Self::Compact => Self::Full,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub id: String,
    pub col: u8,
    pub row: u8,
    pub width: u8,
    pub height: u8,
    pub val: String,
    pub is_submit: bool,
}

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
            field_bg: 1,
            submit_fg: 0,
            submit_bg: 7,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logger with warnings/errors only to avoid cluttering raw stdout
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    println!("Connecting to virtual radio transport at 127.0.0.1:8088...");
    // Create client transport connecting to port 8088
    let client_key = [1u8; 32];
    let transport = Arc::new(MockSocketTransport::new_client(
        "127.0.0.1:8088".to_string(),
        0.0,
        0,
        200,
    ));

    // Send connection handshake packet to boot session
    let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
    let handshake_payloads = handshake_msg.to_fragments(200).unwrap();

    // Sleep a short moment to ensure the TCP connection completes before sending handshake
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    if !handshake_payloads.is_empty() {
        let handshake = RadioPacket {
            is_broadcast: false,
            src_node: client_key,
            dst_node: [0; 32],
            payload: handshake_payloads[0].clone(),
            signal_rssi: -50,
            signal_snr: 10,
        };
        transport.send_packet(handshake).await?;
    }

    // Enable raw mode for single-keystroke interactivity
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.flush()?;

    let layout = Arc::new(Mutex::new(LayoutMode::Full));
    let form_state = Arc::new(Mutex::new(FormState::default()));
    let redraw_trigger = Arc::new(AtomicBool::new(false));

    // Draw initial layout border
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
    let (col_offset, row_offset, w, h) = get_viewport_offsets(LayoutMode::Full, term_w, term_h);
    draw_viewport_border(col_offset, row_offset, w, h);
    print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);
    stdout.flush()?;

    let transport_clone = transport.clone();
    let layout_clone = layout.clone();
    let form_clone = form_state.clone();
    let redraw_clone = redraw_trigger.clone();
    let client_key_clone = client_key;

    let (key_tx, mut key_rx) = tokio::sync::mpsc::channel::<KeyEvent>(100);
    std::thread::spawn(move || loop {
        if let Ok(Event::Key(key_event)) = event::read() {
            if key_tx.blocking_send(key_event).is_err() {
                break;
            }
        }
    });

    // Spawn task to read keyboard events and send to server
    let input_handle = tokio::spawn(async move {
        while let Some(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = key_rx.recv().await
        {
            if kind == event::KeyEventKind::Release {
                continue;
            }
            match code {
                KeyCode::Char('l') | KeyCode::Char('L')
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    // Cycle layout!
                    let mut layout_val = layout_clone.lock().unwrap();
                    *layout_val = layout_val.cycle();
                    redraw_clone.store(true, Ordering::SeqCst);
                }
                KeyCode::Char('x') | KeyCode::Char('X')
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    break;
                }
                KeyCode::Char('c') | KeyCode::Char('C')
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    break;
                }
                KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                    let mut form = form_clone.lock().unwrap();
                    let layout_val = *layout_clone.lock().unwrap();
                    if form.active && !form.fields.is_empty() {
                        form.active_idx = (form.active_idx + 1) % form.fields.len();
                        let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
                        let (col_offset, row_offset, _, _) =
                            get_viewport_offsets(layout_val, term_w, term_h);
                        render_form_fields_offset(&form, col_offset, row_offset);
                        position_cursor(&form, layout_val);
                        let _ = io::stdout().flush();
                    }
                }
                KeyCode::Up | KeyCode::Left => {
                    let mut form = form_clone.lock().unwrap();
                    let layout_val = *layout_clone.lock().unwrap();
                    if form.active && !form.fields.is_empty() {
                        form.active_idx =
                            (form.active_idx + form.fields.len() - 1) % form.fields.len();
                        let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
                        let (col_offset, row_offset, _, _) =
                            get_viewport_offsets(layout_val, term_w, term_h);
                        render_form_fields_offset(&form, col_offset, row_offset);
                        position_cursor(&form, layout_val);
                        let _ = io::stdout().flush();
                    }
                }
                KeyCode::Backspace => {
                    let mut form = form_clone.lock().unwrap();
                    let layout_val = *layout_clone.lock().unwrap();
                    if form.active && !form.fields.is_empty() {
                        let idx = form.active_idx;
                        let fg = form.field_fg;
                        let bg = form.field_bg;
                        let field = &mut form.fields[idx];
                        if !field.is_submit && !field.val.is_empty() {
                            field.val.pop();
                            render_field_local(field, fg, bg, layout_val);
                            position_cursor(&form, layout_val);
                            let _ = io::stdout().flush();
                        }
                    }
                }
                KeyCode::Char(c) => {
                    let form_active_and_field_update = {
                        let mut form = form_clone.lock().unwrap();
                        let layout_val = *layout_clone.lock().unwrap();
                        if form.active && !form.fields.is_empty() {
                            let idx = form.active_idx;
                            let fg = form.field_fg;
                            let bg = form.field_bg;
                            let field = &mut form.fields[idx];
                            let max_len = field.width as usize * field.height as usize;
                            if !field.is_submit && field.val.len() < max_len {
                                field.val.push(c);
                                render_field_local(field, fg, bg, layout_val);
                                position_cursor(&form, layout_val);
                                let _ = io::stdout().flush();
                            }
                            true
                        } else {
                            false
                        }
                    };
                    if !form_active_and_field_update {
                        // Default non-form character input
                        let msg = MeshBbsMessage::new(0x02, 0x02, 0x00, vec![c as u8]);
                        let mtu = transport_clone.get_mtu();
                        if let Ok(fragments) = msg.to_fragments(mtu) {
                            for frag in fragments {
                                let packet = RadioPacket {
                                    is_broadcast: false,
                                    src_node: client_key_clone,
                                    dst_node: [0; 32],
                                    payload: frag,
                                    signal_rssi: -50,
                                    signal_snr: 10,
                                };
                                if transport_clone.send_packet(packet).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    let (should_submit, json_to_send) = {
                        let mut form = form_clone.lock().unwrap();
                        let layout_val = *layout_clone.lock().unwrap();
                        if form.active && !form.fields.is_empty() {
                            let idx = form.active_idx;
                            let active_field = form.fields[idx].clone();
                            if active_field.is_submit {
                                // Compile JSON
                                let mut map = std::collections::HashMap::new();
                                for f in &form.fields {
                                    if !f.is_submit {
                                        map.insert(f.id.clone(), f.val.clone());
                                    }
                                }
                                map.insert("submit".to_string(), active_field.id.clone());
                                let json = serde_json::to_string(&map).unwrap();

                                form.active = false;
                                form.fields.clear();
                                (true, Some(json.into_bytes()))
                            } else {
                                // Move to next field
                                form.active_idx = (form.active_idx + 1) % form.fields.len();
                                let (term_w, term_h) =
                                    crossterm::terminal::size().unwrap_or((80, 25));
                                let (col_offset, row_offset, _, _) =
                                    get_viewport_offsets(layout_val, term_w, term_h);
                                render_form_fields_offset(&form, col_offset, row_offset);
                                position_cursor(&form, layout_val);
                                let _ = io::stdout().flush();
                                (false, None)
                            }
                        } else {
                            (false, None)
                        }
                    };

                    if should_submit {
                        if let Some(payload) = json_to_send {
                            let msg = MeshBbsMessage::new(0x02, 0x02, 0x00, payload);
                            let mtu = transport_clone.get_mtu();
                            if let Ok(fragments) = msg.to_fragments(mtu) {
                                for frag in fragments {
                                    let packet = RadioPacket {
                                        is_broadcast: false,
                                        src_node: client_key_clone,
                                        dst_node: [0; 32],
                                        payload: frag,
                                        signal_rssi: -50,
                                        signal_snr: 10,
                                    };
                                    let _ = transport_clone.send_packet(packet).await;
                                }
                            }
                        }
                    } else {
                        let form_active = {
                            let form = form_clone.lock().unwrap();
                            form.active
                        };
                        if !form_active {
                            // Default non-form Enter key
                            let msg = MeshBbsMessage::new(0x02, 0x02, 0x00, vec![b'\n']);
                            let mtu = transport_clone.get_mtu();
                            if let Ok(fragments) = msg.to_fragments(mtu) {
                                for frag in fragments {
                                    let packet = RadioPacket {
                                        is_broadcast: false,
                                        src_node: client_key_clone,
                                        dst_node: [0; 32],
                                        payload: frag,
                                        signal_rssi: -50,
                                        signal_snr: 10,
                                    };
                                    if transport_clone.send_packet(packet).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                KeyCode::Esc => {
                    break;
                }
                _ => {}
            }
        }
    });

    // Receive and render bytecode packets from server in main loop
    let mut reassembler = MessageReassembler::new();
    let mut bytecode_history = Vec::new();
    loop {
        if input_handle.is_finished() {
            break;
        }

        if redraw_trigger.load(Ordering::SeqCst) {
            redraw_trigger.store(false, Ordering::SeqCst);

            print!("\x1b[2J\x1b[H");
            let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
            let layout_val = *layout.lock().unwrap();
            let (col_offset, row_offset, w, h) = get_viewport_offsets(layout_val, term_w, term_h);
            draw_viewport_border(col_offset, row_offset, w, h);

            let mut form_lock = form_state.lock().unwrap();
            interpret_bytecode(&bytecode_history, &mut form_lock, col_offset, row_offset);

            if form_lock.active {
                position_cursor(&form_lock, layout_val);
            }
            let _ = io::stdout().flush();
        }

        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            transport.receive_packet(),
        )
        .await
        {
            Ok(Ok(packet)) => match reassembler.process_packet([0; 32], &packet.payload) {
                Ok(Some(msg)) => {
                    append_to_history(&mut bytecode_history, &msg.payload);

                    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
                    let layout_val = *layout.lock().unwrap();
                    let (col_offset, row_offset, w, h) =
                        get_viewport_offsets(layout_val, term_w, term_h);

                    if msg.payload.first() == Some(&0x01) {
                        print!("\x1b[2J\x1b[H");
                        draw_viewport_border(col_offset, row_offset, w, h);
                    }

                    let mut form_lock = form_state.lock().unwrap();
                    interpret_bytecode(&msg.payload, &mut form_lock, col_offset, row_offset);

                    if form_lock.active {
                        position_cursor(&form_lock, layout_val);
                    }
                    let _ = io::stdout().flush();
                }
                Ok(None) => {}
                Err(e) => {
                    log::error!("Client packet reassembly error: {}", e);
                }
            },
            Ok(Err(meshcore_transport::TransportError::ConnectionClosed)) => {
                break;
            }
            Ok(Err(_)) => {}
            Err(_) => {} // timeout
        }
    }

    let _ = disable_raw_mode();
    stdout.execute(LeaveAlternateScreen)?;
    stdout.flush()?;
    println!("Disconnected from BBS.");
    Ok(())
}

fn get_viewport_offsets(
    layout: LayoutMode,
    term_width: u16,
    term_height: u16,
) -> (u16, u16, u16, u16) {
    let (w, h) = match layout {
        LayoutMode::Full => (80, 25),
        LayoutMode::Compact => (40, 25),
    };
    let col_offset = if term_width > w {
        (term_width - w) / 2
    } else {
        0
    };
    let row_offset = if term_height > h {
        (term_height - h) / 2
    } else {
        0
    };
    (col_offset, row_offset, w, h)
}

fn draw_viewport_border(col_offset: u16, row_offset: u16, w: u16, h: u16) {
    if col_offset == 0 || row_offset == 0 {
        return;
    }
    // Draw top border
    print!("\x1b[{};{}H+", row_offset, col_offset);
    for _ in 0..w {
        print!("-");
    }
    print!("+");

    // Draw side borders
    for r in 0..h {
        print!("\x1b[{};{}H|", row_offset + 1 + r, col_offset);
        print!("\x1b[{};{}H|", row_offset + 1 + r, col_offset + 1 + w);
    }

    // Draw bottom border
    print!("\x1b[{};{}H+", row_offset + 1 + h, col_offset);
    for _ in 0..w {
        print!("-");
    }
    print!("+");
}

fn position_cursor(form: &FormState, layout: LayoutMode) {
    if form.fields.is_empty() {
        return;
    }
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
    let (col_offset, row_offset, _, _) = get_viewport_offsets(layout, term_w, term_h);
    let field = &form.fields[form.active_idx];
    if field.is_submit {
        print!(
            "\x1b[{};{}H",
            row_offset + field.row as u16 + 1,
            col_offset + field.col as u16 + 3
        );
    } else {
        let char_idx = field.val.chars().count();
        let r = char_idx / field.width as usize;
        let c = char_idx % field.width as usize;
        print!(
            "\x1b[{};{}H",
            row_offset + field.row as u16 + r as u16 + 1,
            col_offset + field.col as u16 + c as u16 + 1
        );
    }
}

fn render_form_fields_offset(form: &FormState, col_offset: u16, row_offset: u16) {
    for (idx, field) in form.fields.iter().enumerate() {
        let is_active = idx == form.active_idx;
        if field.is_submit {
            let (fg, bg) = if is_active {
                // Focus highlight: swap colors
                (form.submit_bg, form.submit_fg)
            } else {
                (form.submit_fg, form.submit_bg)
            };
            apply_color_attribute((bg << 4) | fg);
            print!(
                "\x1b[{};{}H[ {} ]\x1b[0m",
                row_offset + field.row as u16 + 1,
                col_offset + field.col as u16 + 1,
                field.id
            );
        } else {
            let (fg, bg) = if is_active {
                // Focus highlight: swap colors
                (form.field_bg, form.field_fg)
            } else {
                (form.field_fg, form.field_bg)
            };
            apply_color_attribute((bg << 4) | fg);
            for r in 0..field.height {
                print!(
                    "\x1b[{};{}H{:width$}",
                    row_offset + field.row as u16 + r as u16 + 1,
                    col_offset + field.col as u16 + 1,
                    "",
                    width = field.width as usize
                );
            }
            let val_chars: Vec<char> = field.val.chars().collect();
            for r in 0..field.height {
                let start = r as usize * field.width as usize;
                if start < val_chars.len() {
                    let end = std::cmp::min(start + field.width as usize, val_chars.len());
                    let line_str: String = val_chars[start..end].iter().collect();
                    print!(
                        "\x1b[{};{}H{}",
                        row_offset + field.row as u16 + r as u16 + 1,
                        col_offset + field.col as u16 + 1,
                        line_str
                    );
                }
            }
            print!("\x1b[0m");
        }
    }
}

fn render_field_local(field: &FormField, fg: u8, bg: u8, layout: LayoutMode) {
    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
    let (col_offset, row_offset, _, _) = get_viewport_offsets(layout, term_w, term_h);
    apply_color_attribute((bg << 4) | fg);
    for r in 0..field.height {
        print!(
            "\x1b[{};{}H{:width$}",
            row_offset + field.row as u16 + r as u16 + 1,
            col_offset + field.col as u16 + 1,
            "",
            width = field.width as usize
        );
    }
    let val_chars: Vec<char> = field.val.chars().collect();
    for r in 0..field.height {
        let start = r as usize * field.width as usize;
        if start < val_chars.len() {
            let end = std::cmp::min(start + field.width as usize, val_chars.len());
            let line_str: String = val_chars[start..end].iter().collect();
            print!(
                "\x1b[{};{}H{}",
                row_offset + field.row as u16 + r as u16 + 1,
                col_offset + field.col as u16 + 1,
                line_str
            );
        }
    }
    print!("\x1b[0m");
}

fn append_to_history(history: &mut Vec<u8>, payload: &[u8]) {
    if let Some(pos) = payload.iter().position(|&x| x == 0x01) {
        history.clear();
        history.extend_from_slice(&payload[pos..]);
    } else {
        history.extend_from_slice(payload);
    }
}

fn interpret_bytecode(
    payload: &[u8],
    form_state: &mut FormState,
    col_offset: u16,
    row_offset: u16,
) {
    let mut i = 0;
    while i < payload.len() {
        let op = payload[i];
        match op {
            0x00 => {
                i += 1;
            }
            0x01 => {
                // OP_CLEAR_SCREEN (Viewport relative)
                print!("\x1b[2J\x1b[H");
                let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
                let (_, _, w, h) = get_viewport_offsets(
                    if col_offset > 25 {
                        LayoutMode::Full
                    } else {
                        LayoutMode::Compact
                    },
                    term_w,
                    term_h,
                );
                draw_viewport_border(col_offset, row_offset, w, h);
                print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);

                // Clear active form state when screen is cleared!
                form_state.active = false;
                form_state.fields.clear();
                form_state.active_idx = 0;

                i += 1;
            }
            0x02 => {
                // OP_CRLF inside viewport
                print!("\r\n\x1b[{}C", col_offset);
                i += 1;
            }
            b'\n' => {
                print!("\r\n\x1b[{}C", col_offset);
                i += 1;
            }
            0x04 => {
                i += 1;
            }
            0xC0 => {
                // OP_SET_COLOR
                if i + 1 < payload.len() {
                    let attr = payload[i + 1];
                    apply_color_attribute(attr);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            0xC3 => {
                // OP_CURSOR_ABS inside viewport
                if i + 2 < payload.len() {
                    let col = payload[i + 1];
                    let row = payload[i + 2];
                    print!(
                        "\x1b[{};{}H",
                        row_offset + row as u16 + 1,
                        col_offset + col as u16 + 1
                    );
                    i += 3;
                } else {
                    i += 1;
                }
            }
            0xC5 => {
                // OP_RENDER_ASSET
                if i + 2 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    render_cached_asset(id, col_offset, row_offset);
                    i += 3;
                } else {
                    i += 1;
                }
            }
            0xD0 => {
                // OP_FORM_START (form_id, field_fg, field_bg, submit_fg, submit_bg)
                if i + 5 < payload.len() {
                    let form_id = payload[i + 1];
                    let field_fg = payload[i + 2];
                    let field_bg = payload[i + 3];
                    let submit_fg = payload[i + 4];
                    let submit_bg = payload[i + 5];

                    form_state.active = true;
                    form_state.form_id = form_id;
                    form_state.field_fg = field_fg;
                    form_state.field_bg = field_bg;
                    form_state.submit_fg = submit_fg;
                    form_state.submit_bg = submit_bg;
                    form_state.fields.clear();
                    form_state.active_idx = 0;
                    i += 6;
                } else {
                    i += 1;
                }
            }
            0xD1 => {
                // OP_FORM_FIELD
                if i + 5 < payload.len() {
                    let col = payload[i + 1];
                    let row = payload[i + 2];
                    let width = payload[i + 3];
                    let id_len = payload[i + 4] as usize;
                    if i + 5 + id_len < payload.len() {
                        let id =
                            String::from_utf8_lossy(&payload[i + 5..i + 5 + id_len]).into_owned();
                        let val_len_idx = i + 5 + id_len;
                        let val_len = payload[val_len_idx] as usize;
                        if val_len_idx + 1 + val_len <= payload.len() {
                            let val = String::from_utf8_lossy(
                                &payload[val_len_idx + 1..val_len_idx + 1 + val_len],
                            )
                            .into_owned();

                            form_state.fields.push(FormField {
                                id,
                                col,
                                row,
                                width,
                                height: 1,
                                val: val.clone(),
                                is_submit: false,
                            });

                            // Render field with form colors
                            apply_color_attribute((form_state.field_bg << 4) | form_state.field_fg);
                            print!(
                                "\x1b[{};{}H{:width$}\x1b[0m",
                                row_offset + row as u16 + 1,
                                col_offset + col as u16 + 1,
                                val,
                                width = width as usize
                            );
                            i = val_len_idx + 1 + val_len;
                        } else {
                            i += 1;
                        }
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            0xD2 => {
                // OP_FORM_SUBMIT
                if i + 4 < payload.len() {
                    let col = payload[i + 1];
                    let row = payload[i + 2];
                    let id_len = payload[i + 3] as usize;
                    if i + 4 + id_len <= payload.len() {
                        let id =
                            String::from_utf8_lossy(&payload[i + 4..i + 4 + id_len]).into_owned();

                        form_state.fields.push(FormField {
                            id: id.clone(),
                            col,
                            row,
                            width: (id.len() + 4) as u8,
                            height: 1,
                            val: String::new(),
                            is_submit: true,
                        });

                        // Render button with form colors
                        apply_color_attribute((form_state.submit_bg << 4) | form_state.submit_fg);
                        print!(
                            "\x1b[{};{}H[ {} ]\x1b[0m",
                            row_offset + row as u16 + 1,
                            col_offset + col as u16 + 1,
                            id
                        );
                        i = i + 4 + id_len;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            0xD3 => {
                // OP_FORM_END
                render_form_fields_offset(form_state, col_offset, row_offset);
                i += 1;
            }
            0xD4 => {
                // OP_FORM_FIELD_MULTILINE (col, row, width, height, id, val)
                if i + 6 < payload.len() {
                    let col = payload[i + 1];
                    let row = payload[i + 2];
                    let width = payload[i + 3];
                    let height = payload[i + 4];
                    let id_len = payload[i + 5] as usize;
                    if i + 6 + id_len < payload.len() {
                        let id =
                            String::from_utf8_lossy(&payload[i + 6..i + 6 + id_len]).into_owned();
                        let val_len_idx = i + 6 + id_len;
                        let val_len = payload[val_len_idx] as usize;
                        if val_len_idx + 1 + val_len <= payload.len() {
                            let val = String::from_utf8_lossy(
                                &payload[val_len_idx + 1..val_len_idx + 1 + val_len],
                            )
                            .into_owned();

                            form_state.fields.push(FormField {
                                id,
                                col,
                                row,
                                width,
                                height,
                                val: val.clone(),
                                is_submit: false,
                            });

                            // Render field with form colors
                            apply_color_attribute((form_state.field_bg << 4) | form_state.field_fg);
                            for r in 0..height {
                                print!(
                                    "\x1b[{};{}H{:width$}\x1b[0m",
                                    row_offset + row as u16 + r as u16 + 1,
                                    col_offset + col as u16 + 1,
                                    "",
                                    width = width as usize
                                );
                            }
                            let val_chars: Vec<char> = val.chars().collect();
                            for r in 0..height {
                                let start = r as usize * width as usize;
                                if start < val_chars.len() {
                                    let end =
                                        std::cmp::min(start + width as usize, val_chars.len());
                                    let line_str: String = val_chars[start..end].iter().collect();
                                    print!(
                                        "\x1b[{};{}H{}",
                                        row_offset + row as u16 + r as u16 + 1,
                                        col_offset + col as u16 + 1,
                                        line_str
                                    );
                                }
                            }

                            i = val_len_idx + 1 + val_len;
                        } else {
                            i += 1;
                        }
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
            c => {
                print!("{}", c as char);
                i += 1;
            }
        }
    }
}

fn apply_color_attribute(attr: u8) {
    let fg = attr & 0x0F;
    let bg = (attr & 0xF0) >> 4;

    let fg_code = if fg < 8 { 30 + fg } else { 90 + (fg - 8) };

    let bg_code = if bg < 8 { 40 + bg } else { 100 + (bg - 8) };

    print!("\x1b[{};{}m", fg_code, bg_code);
}

fn render_cached_asset(asset_id: u16, col_offset: u16, row_offset: u16) {
    let path = match asset_id {
        0x0101 => Some("assets/dungeon_banner.ans"),
        0x0102 => Some("assets/main_menu_border.ans"),
        0x0103 => Some("assets/main_menu_banner.ans"),
        _ => None,
    };
    if let Some(p) = path {
        if let Ok(content) = std::fs::read_to_string(p) {
            print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);
            let newline_replacement = format!("\r\n\x1b[{}C", col_offset);
            let aligned_content = content
                .replace("\r\n", "\n")
                .replace("\n", &newline_replacement);
            print!("{}", aligned_content);
        }
    }
}
