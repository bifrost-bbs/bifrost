//! Interactive client terminal emulator for MeshBBS testing over mock radio socket.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bifrost_transport::{
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
    pub key: Option<char>,
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
            field_bg: 4,
            submit_fg: 0,
            submit_bg: 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub log_level: Option<String>,
    pub auto_crawl: bool,
    pub crawl_steps: usize,
    pub crawl_delay_ms: u64,
    pub headless: bool,
    pub server_addr: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            log_level: None,
            auto_crawl: false,
            crawl_steps: 50,
            crawl_delay_ms: 250,
            headless: false,
            server_addr: "127.0.0.1:8088".to_string(),
        }
    }
}

pub fn parse_cli_args(args: &[String]) -> Result<Option<ClientConfig>> {
    let mut config = ClientConfig::default();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--log-level" | "-l" => {
                if i + 1 < args.len() {
                    config.log_level = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--debug" | "-v" | "--verbose" => {
                config.log_level = Some("debug".to_string());
            }
            "--trace" => {
                config.log_level = Some("trace".to_string());
            }
            "--auto-navigate" | "--crawl" | "--auto" => {
                config.auto_crawl = true;
            }
            "--crawl-steps" | "-n" => {
                if i + 1 < args.len() {
                    if let Ok(steps) = args[i + 1].parse::<usize>() {
                        config.crawl_steps = steps;
                    }
                    i += 1;
                }
            }
            "--crawl-delay" | "-d" => {
                if i + 1 < args.len() {
                    if let Ok(delay) = args[i + 1].parse::<u64>() {
                        config.crawl_delay_ms = delay;
                    }
                    i += 1;
                }
            }
            "--headless" => {
                config.headless = true;
            }
            "--addr" => {
                if i + 1 < args.len() {
                    config.server_addr = args[i + 1].clone();
                    i += 1;
                }
            }
            "--help" | "-h" => {
                println!("Usage: bifrost-client [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -l, --log-level <LEVEL>    Set log level (trace, debug, info, warn, error)");
                println!("  -v, --debug, --verbose     Enable debug logging");
                println!("      --trace                Enable trace logging");
                println!("      --crawl, --auto-navigate Automate navigating BBS options at random");
                println!("  -n, --crawl-steps <STEPS>  Number of crawl steps to execute (default: 50)");
                println!("  -d, --crawl-delay <MS>     Delay in milliseconds between actions (default: 250)");
                println!("      --headless             Run headless without interactive terminal display");
                println!("      --addr <IP:PORT>       Server address to connect to (default: 127.0.0.1:8088)");
                println!("  -h, --help                 Print help");
                return Ok(None);
            }
            _ => {}
        }
        i += 1;
    }
    Ok(Some(config))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlAction {
    pub payload: Vec<u8>,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct CrawlState {
    pub visit_counts: HashMap<(u8, String), usize>, // (form_id, button_id) -> count
    pub last_form_id: Option<u8>,
    pub last_button_id: Option<String>,
    pub rng_state: u64,
}

impl Default for CrawlState {
    fn default() -> Self {
        let initial_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);

        Self {
            visit_counts: HashMap::new(),
            last_form_id: None,
            last_button_id: None,
            rng_state: initial_seed ^ 0x6C62272E07BB0142,
        }
    }
}

impl CrawlState {
    pub fn next_rand(&mut self) -> u64 {
        // XorShift64* PRNG
        self.rng_state ^= self.rng_state >> 12;
        self.rng_state ^= self.rng_state << 25;
        self.rng_state ^= self.rng_state >> 27;
        self.rng_state.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn decide_action(&mut self, form: &mut FormState) -> CrawlAction {
        if form.active && !form.fields.is_empty() {
            let submit_indices: Vec<usize> = form
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| f.is_submit)
                .map(|(i, _)| i)
                .collect();

            if !submit_indices.is_empty() {
                // Determine weights for each submit button
                // 1. Base weight inversely proportional to prior visit count
                // 2. Penalize "back", "cancel", "main_menu", "logout" if there are other unvisited options
                // 3. Avoid immediately repeating the action that brought us here
                let mut weights = Vec::with_capacity(submit_indices.len());
                let has_forward_options = submit_indices.iter().any(|&i| {
                    let id = &form.fields[i].id;
                    id != "back" && id != "cancel" && id != "logout" && id != "main_menu"
                });

                for &idx in &submit_indices {
                    let field = &form.fields[idx];
                    let prior_visits = *self.visit_counts.get(&(form.form_id, field.id.clone())).unwrap_or(&0);
                    
                    // High weight for least visited items: 1000 / (1 + visits * 4)
                    let mut w: f64 = 1000.0 / (1.0 + (prior_visits as f64) * 4.0);

                    let is_exit_button = field.id == "back" || field.id == "cancel" || field.id == "main_menu" || field.id == "logout";
                    if is_exit_button && has_forward_options {
                        // If other forward options exist on this form that haven't been visited,
                        // heavily discount the back/exit button to encourage exploration
                        let unvisited_forward_count = submit_indices.iter().filter(|&&i| {
                            let fid = &form.fields[i].id;
                            fid != "back" && fid != "cancel" && fid != "logout" && fid != "main_menu"
                                && *self.visit_counts.get(&(form.form_id, fid.clone())).unwrap_or(&0) == 0
                        }).count();

                        if unvisited_forward_count > 0 {
                            w *= 0.05; // 95% discount on early back-tracking
                        } else {
                            w *= 0.40; // Moderate discount once some forward options explored
                        }
                    }

                    if field.id == "logout" {
                        w *= 0.01; // Very rare logout so session continues exploring
                    }

                    // Anti-ping-pong: If we just clicked "back" from Form A to Form B,
                    // don't immediately re-click the exact same button that took us to Form A
                    if let (Some(_last_fid), Some(ref last_bid)) = (self.last_form_id, &self.last_button_id) {
                        if last_bid == "back" && self.visit_counts.get(&(form.form_id, field.id.clone())).unwrap_or(&0) > &0 {
                            w *= 0.15;
                        }
                    }

                    weights.push(w.max(0.001));
                }

                // Weighted random roulette selection
                let total_weight: f64 = weights.iter().sum();
                let rand_val = ((self.next_rand() % 10000) as f64 / 10000.0) * total_weight;

                let mut cumulative = 0.0;
                let mut chosen_idx = submit_indices[0];
                for (i, &w) in weights.iter().enumerate() {
                    cumulative += w;
                    if rand_val <= cumulative {
                        chosen_idx = submit_indices[i];
                        break;
                    }
                }

                let chosen_field = form.fields[chosen_idx].clone();
                *self.visit_counts.entry((form.form_id, chosen_field.id.clone())).or_insert(0) += 1;
                self.last_form_id = Some(form.form_id);
                self.last_button_id = Some(chosen_field.id.clone());

                let mut map = HashMap::new();
                for f in &form.fields {
                    if !f.is_submit {
                        map.insert(f.id.clone(), f.val.clone());
                    }
                }
                map.insert("submit".to_string(), chosen_field.id.clone());
                let json = serde_json::to_string(&map).unwrap();
                let desc = format!("Submit button '{}' on Form {}", chosen_field.id, form.form_id);

                form.active = false;
                form.fields.clear();
                CrawlAction {
                    payload: json.into_bytes(),
                    description: desc,
                }
            } else {
                // Form with only input fields (no submit button): preserve values and submit
                let mut map = HashMap::new();
                for f in &form.fields {
                    map.insert(f.id.clone(), f.val.clone());
                }
                let json = serde_json::to_string(&map).unwrap();
                let desc = format!("Submit form {} fields", form.form_id);

                form.active = false;
                form.fields.clear();
                CrawlAction {
                    payload: json.into_bytes(),
                    description: desc,
                }
            }
        } else {
            // Plain text screen / prompt
            let keys = [b'\r', b' ', b'1', b'2', b'3', b'b', b'\r'];
            let idx = (self.next_rand() as usize) % keys.len();
            let chosen_key = keys[idx];
            let desc = format!("Keystroke {:?}", chosen_key as char);
            CrawlAction {
                payload: vec![chosen_key],
                description: desc,
            }
        }
    }
}

pub fn decide_crawl_action(form: &mut FormState, rng_seed: usize) -> CrawlAction {
    let mut state = CrawlState {
        visit_counts: HashMap::new(),
        last_form_id: None,
        last_button_id: None,
        rng_state: (rng_seed as u64) | 1,
    };
    state.decide_action(form)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let config = match parse_cli_args(&args)? {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    // Default to warn for client unless configured via CLI or RUST_LOG
    let default_level = config.log_level.unwrap_or_else(|| "warn".to_string());
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level)).init();

    println!("Connecting to virtual radio transport at {}...", config.server_addr);
    // Create client transport connecting to specified port
    let client_key = [1u8; 32];
    let transport = Arc::new(MockSocketTransport::new_client(
        config.server_addr.clone(),
        0.0,
        0,
        200,
    ));

    // Sleep a short moment to ensure the TCP connection completes before sending advert/handshake
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Send a mock binary advert packet so the BBS learns our metadata (name, location)
    let mut advert_payload = Vec::new();
    advert_payload.extend_from_slice(&client_key); // 32 bytes public key
    advert_payload.extend_from_slice(&0u32.to_le_bytes()); // 4 bytes timestamp
    advert_payload.extend_from_slice(&[0u8; 64]); // 64 bytes signature

    let flags: u8 = 0x80 | 0x10; // has name | has location
    advert_payload.push(flags);

    // latitude (37.7749 * 1_000_000)
    let lat_int: i32 = 37774900;
    advert_payload.extend_from_slice(&lat_int.to_le_bytes());
    // longitude (-122.4194 * 1_000_000)
    let lon_int: i32 = -122419400;
    advert_payload.extend_from_slice(&lon_int.to_le_bytes());

    let node_name = "TestClient";
    advert_payload.extend_from_slice(node_name.as_bytes());

    let advert_packet = RadioPacket {
        is_broadcast: true,
        src_node: client_key,
        dst_node: [0; 32],
        payload: advert_payload,
        signal_rssi: -40,
        signal_snr: 12,
    };
    transport.send_packet(advert_packet).await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Send connection handshake packet to boot session
    let handshake_msg = MeshBbsMessage::new(0x03, 0x01, 0x00, Vec::new());
    let handshake_payloads = handshake_msg.to_fragments(200).unwrap();
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

    let shutdown_signal = Arc::new(AtomicBool::new(false));

    // Terminal layout and state setup
    let layout = Arc::new(Mutex::new(LayoutMode::Full));
    let form_state = Arc::new(Mutex::new(FormState::default()));
    let redraw_trigger = Arc::new(AtomicBool::new(false));

    if !config.headless {
        // Enable raw mode for single-keystroke interactivity
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.execute(EnterAlternateScreen)?;
        stdout.flush()?;

        // Draw initial layout border
        let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
        let (col_offset, row_offset, w, h) = get_viewport_offsets(LayoutMode::Full, term_w, term_h);
        draw_viewport_border(col_offset, row_offset, w, h);
        print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);
        stdout.flush()?;
    }

    let transport_clone = transport.clone();
    let layout_clone = layout.clone();
    let form_clone = form_state.clone();
    let redraw_clone = redraw_trigger.clone();
    let client_key_clone = client_key;
    let shutdown_clone = shutdown_signal.clone();

    // Spawn automated crawler task if enabled
    if config.auto_crawl {
        let form_crawl = form_state.clone();
        let transport_crawl = transport.clone();
        let client_key_crawl = client_key;
        let shutdown_crawl = shutdown_signal.clone();
        let steps_total = config.crawl_steps;
        let delay_ms = config.crawl_delay_ms;
        let is_headless = config.headless;

        tokio::spawn(async move {
            log::info!("[AUTO-CRAWLER] Started automated navigation ({} steps, {}ms delay)", steps_total, delay_ms);
            if is_headless {
                println!("[AUTO-CRAWLER] Started automated navigation ({} steps, {}ms delay)...", steps_total, delay_ms);
            }

            // Allow initial handshake and screen load to arrive
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let mut crawler = CrawlState::default();
            let mut step_count = 0;

            while step_count < steps_total || steps_total == 0 {
                if shutdown_crawl.load(Ordering::SeqCst) {
                    break;
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                step_count += 1;

                let action = {
                    let mut form = form_crawl.lock().unwrap();
                    crawler.decide_action(&mut form)
                };

                log::info!("[AUTO-CRAWLER] Step {}/{}: {}", step_count, steps_total, action.description);
                if is_headless {
                    println!("[AUTO-CRAWLER] Step {}/{}: {}", step_count, steps_total, action.description);
                }

                let msg = MeshBbsMessage::new(0x02, 0x02, 0x00, action.payload);
                let mtu = transport_crawl.get_mtu();
                if let Ok(frags) = msg.to_fragments(mtu) {
                    for frag in frags {
                        let packet = RadioPacket {
                            is_broadcast: false,
                            src_node: client_key_crawl,
                            dst_node: [0; 32],
                            payload: frag,
                            signal_rssi: -50,
                            signal_snr: 10,
                        };
                        if transport_crawl.send_packet(packet).await.is_err() {
                            break;
                        }
                    }
                }
            }

            log::info!("[AUTO-CRAWLER] Completed {} automated steps.", step_count);
            if is_headless {
                println!("[AUTO-CRAWLER] Completed {} automated steps.", step_count);
            }
            shutdown_crawl.store(true, Ordering::SeqCst);
        });
    }

    // Keyboard input handling (interactive mode)
    if !config.headless {
        let (key_tx, mut key_rx) = tokio::sync::mpsc::channel::<KeyEvent>(100);
        std::thread::spawn(move || loop {
            if let Ok(Event::Key(key_event)) = event::read() {
                if key_tx.blocking_send(key_event).is_err() {
                    break;
                }
            }
        });

        // Spawn task to read keyboard events and send to server
        tokio::spawn(async move {
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
                        shutdown_clone.store(true, Ordering::SeqCst);
                        break;
                    }
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        shutdown_clone.store(true, Ordering::SeqCst);
                        break;
                    }
                    KeyCode::Esc => {
                        shutdown_clone.store(true, Ordering::SeqCst);
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
                    let mut should_submit_json = None;
                    let handled = {
                        let mut form = form_clone.lock().unwrap();
                        let layout_val = *layout_clone.lock().unwrap();
                        if form.active && !form.fields.is_empty() {
                            let idx = form.active_idx;
                            let fg = form.field_fg;
                            let bg = form.field_bg;
                            let current_is_submit = form.fields[idx].is_submit;

                            if !current_is_submit {
                                let field = &mut form.fields[idx];
                                let max_len = field.width as usize * field.height as usize;
                                if field.val.len() < max_len {
                                    field.val.push(c);
                                    render_field_local(field, fg, bg, layout_val);
                                    position_cursor(&form, layout_val);
                                    let _ = io::stdout().flush();
                                }
                                true
                            } else {
                                // Submit / menu button mode: check hotkeys!
                                let lower_c = c.to_ascii_lowercase();
                                let mut matched_idx = None;
                                for (i, f) in form.fields.iter().enumerate() {
                                    if f.is_submit {
                                        if let Some(k) = f.key {
                                            if k.to_ascii_lowercase() == lower_c {
                                                matched_idx = Some(i);
                                                break;
                                            }
                                        }
                                        let label_first = f.val.chars().next().map(|ch| ch.to_ascii_lowercase());
                                        let id_first = f.id.chars().next().map(|ch| ch.to_ascii_lowercase());
                                        if label_first == Some(lower_c) || id_first == Some(lower_c) {
                                            matched_idx = Some(i);
                                            break;
                                        }
                                    }
                                }

                                if let Some(target_idx) = matched_idx {
                                    form.active_idx = target_idx;
                                    let target_field = form.fields[target_idx].clone();

                                    let mut map = std::collections::HashMap::new();
                                    for f in &form.fields {
                                        if !f.is_submit {
                                            map.insert(f.id.clone(), f.val.clone());
                                        }
                                    }
                                    map.insert("submit".to_string(), target_field.id.clone());
                                    let json = serde_json::to_string(&map).unwrap();

                                    form.active = false;
                                    form.fields.clear();
                                    should_submit_json = Some(json.into_bytes());
                                    true
                                } else {
                                    false
                                }
                            }
                        } else {
                            false
                        }
                    };

                    if let Some(payload) = should_submit_json {
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
                    } else if !handled {
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
                _ => {}
            }
        }
    });
}

    // Receive and render bytecode packets from server in main loop
    let mut reassembler = MessageReassembler::new();
    let mut bytecode_history = Vec::new();
    let (req_asset_tx, mut req_asset_rx) = tokio::sync::mpsc::unbounded_channel::<u16>();
    let mut asset_cache_assembler: HashMap<u16, (u8, u32, HashMap<u8, Vec<u8>>)> = HashMap::new();
    let mut connected_bbs_node: [u8; 32] = [0; 32];
    let mut client_dict = load_client_dictionary(&connected_bbs_node);
    let mut client_session_cache = bifrost_transport::SessionPayloadCache::new(100);

    loop {
        if shutdown_signal.load(Ordering::SeqCst) {
            break;
        }

        // Drain any pending asset requests
        while let Ok(asset_id) = req_asset_rx.try_recv() {
            log::info!("Sending REQ_ASSET for asset 0x{:04X} to server", asset_id);
            let req_msg = MeshBbsMessage::new(0x01, 0x05, 0x00, asset_id.to_be_bytes().to_vec());
            let mtu = transport.get_mtu();
            if let Ok(frags) = req_msg.to_fragments(mtu) {
                for frag in frags {
                    let packet = RadioPacket {
                        is_broadcast: false,
                        src_node: client_key,
                        dst_node: [0; 32],
                        payload: frag,
                        signal_rssi: -50,
                        signal_snr: 10,
                    };
                    let _ = transport.send_packet(packet).await;
                }
            }
        }

        if redraw_trigger.load(Ordering::SeqCst) {
            redraw_trigger.store(false, Ordering::SeqCst);

            print!("\x1b[2J\x1b[H");
            let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
            let layout_val = *layout.lock().unwrap();
            let (col_offset, row_offset, w, h) = get_viewport_offsets(layout_val, term_w, term_h);
            draw_viewport_border(col_offset, row_offset, w, h);

            let mut form_lock = form_state.lock().unwrap();
            interpret_bytecode(
                &connected_bbs_node,
                &bytecode_history,
                &mut form_lock,
                layout_val,
                col_offset,
                row_offset,
                &req_asset_tx,
            );

            if form_lock.active {
                position_cursor(&form_lock, layout_val);
            }
            let _ = io::stdout().flush();
        }

        if shutdown_signal.load(Ordering::SeqCst) {
            break;
        }

        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            transport.receive_packet(),
        )
        .await
        {
            Ok(Ok(packet)) => {
                if !packet.is_broadcast && packet.dst_node != [0; 32] && packet.dst_node != client_key {
                    continue;
                }

                if packet.src_node != [0; 32] && packet.src_node != connected_bbs_node {
                    connected_bbs_node = packet.src_node;
                    client_dict = load_client_dictionary(&connected_bbs_node);
                }

                // Check for public broadcast asset chunks (AppPort 0xBB, MsgType 0x04)
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
                    if packet.payload.len() >= 12 + payload_len {
                        let chunk_data = &packet.payload[12..12 + payload_len];
                        let entry = asset_cache_assembler
                            .entry(asset_id)
                            .or_insert_with(|| (total_chunks, master_crc, HashMap::new()));
                        entry.2.insert(chunk_idx, chunk_data.to_vec());
                        if entry.2.len() == total_chunks as usize {
                            let mut assembled = Vec::new();
                            for idx in 1..=total_chunks {
                                if let Some(c) = entry.2.get(&idx) {
                                    assembled.extend_from_slice(c);
                                }
                            }
                            if bifrost_transport::crc32(&assembled) == master_crc {
                                log::info!(
                                    "Promiscuous cache assembled asset 0x{:04X} ({} bytes)",
                                    asset_id,
                                    assembled.len()
                                );
                                save_asset_to_cache(&connected_bbs_node, asset_id, &assembled);
                                if asset_id == 0x00DF {
                                    client_dict = load_client_dictionary(&connected_bbs_node);
                                }
                                redraw_trigger.store(true, Ordering::SeqCst);
                            } else {
                                log::warn!("Asset 0x{:04X} CRC32 mismatch, discarding", asset_id);
                            }
                        }
                    }
                    continue;
                }

                match reassembler.process_packet([0; 32], &packet.payload) {
                    Ok(Some(msg)) => {
                        let payload = if (msg.flags & 0x08) != 0 {
                            // Hash-referencing previous session payload
                            if msg.payload.len() >= 4 {
                                let crc = u32::from_be_bytes([
                                    msg.payload[0],
                                    msg.payload[1],
                                    msg.payload[2],
                                    msg.payload[3],
                                ]);
                                if let Some(cached) = client_session_cache.get(crc) {
                                    log::debug!(
                                        "[SESSION DEDUP] Cache hit for CRC 0x{:08X} ({} bytes recovered)",
                                        crc,
                                        cached.len()
                                    );
                                    transport
                                        .stats
                                        .record_decompression(msg.payload.len(), cached.len());
                                    cached.clone()
                                } else {
                                    log::warn!(
                                        "[SESSION DEDUP] Cache miss for CRC 0x{:08X}, sending NACK to BBS",
                                        crc
                                    );
                                    let nack_msg = MeshBbsMessage::new(
                                        0x01,
                                        0x06,
                                        0x00,
                                        crc.to_be_bytes().to_vec(),
                                    );
                                    let mtu = transport.get_mtu();
                                    if let Ok(frags) = nack_msg.to_fragments(mtu) {
                                        for frag in frags {
                                            let packet = RadioPacket {
                                                is_broadcast: false,
                                                src_node: client_key,
                                                dst_node: connected_bbs_node,
                                                payload: frag,
                                                signal_rssi: -50,
                                                signal_snr: 10,
                                            };
                                            let _ = transport.send_packet(packet).await;
                                        }
                                    }
                                    continue;
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
                                    transport
                                        .stats
                                        .record_decompression(msg.payload.len(), decomp.len());
                                    let crc = bifrost_transport::crc32(&decomp);
                                    client_session_cache.insert(crc, decomp.clone());
                                    decomp
                                }
                                Err(e) => {
                                    log::error!("Failed to decompress bytecode: {:?}", e);
                                    msg.payload
                                }
                            }
                        } else {
                            let crc = bifrost_transport::crc32(&msg.payload);
                            client_session_cache.insert(crc, msg.payload.clone());
                            msg.payload
                        };

                        append_to_history(&mut bytecode_history, &payload);

                        let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 25));
                        let layout_val = *layout.lock().unwrap();
                        let (col_offset, row_offset, w, h) =
                            get_viewport_offsets(layout_val, term_w, term_h);

                        if !config.headless && payload.first() == Some(&0x01) {
                            print!("\x1b[2J\x1b[H");
                            draw_viewport_border(col_offset, row_offset, w, h);
                        }

                        let mut form_lock = form_state.lock().unwrap();
                        if config.headless {
                            interpret_bytecode_headless(
                                &connected_bbs_node,
                                &payload,
                                &mut form_lock,
                                &req_asset_tx,
                            );
                        } else {
                            interpret_bytecode(
                                &connected_bbs_node,
                                &payload,
                                &mut form_lock,
                                layout_val,
                                col_offset,
                                row_offset,
                                &req_asset_tx,
                            );

                            if form_lock.active {
                                position_cursor(&form_lock, layout_val);
                            }
                            let _ = io::stdout().flush();
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::error!("Client packet reassembly error: {}", e);
                    }
                }
            }
            Ok(Err(bifrost_transport::TransportError::ConnectionClosed)) => {
                break;
            }
            Ok(Err(_)) => {}
            Err(_) => {} // timeout
        }
    }

    if !config.headless {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.execute(LeaveAlternateScreen);
        let _ = stdout.flush();
    }
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
            let label = if !field.val.is_empty() {
                &field.val
            } else {
                &field.id
            };
            print!(
                "\x1b[{};{}H[ {} ]\x1b[0m",
                row_offset + field.row as u16 + 1,
                col_offset + field.col as u16 + 1,
                label
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
    server_node_id: &[u8; 32],
    payload: &[u8],
    form_state: &mut FormState,
    layout: LayoutMode,
    col_offset: u16,
    row_offset: u16,
    req_asset_tx: &tokio::sync::mpsc::UnboundedSender<u16>,
) {
    let (max_w, _max_h) = match layout {
        LayoutMode::Full => (80, 25),
        LayoutMode::Compact => (40, 25),
    };
    let mut cur_col: u16 = 0;
    let mut cur_row: u16 = 0;
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
                let (c_off, r_off, w, h) = get_viewport_offsets(layout, term_w, term_h);
                draw_viewport_border(c_off, r_off, w, h);
                print!("\x1b[{};{}H", r_off + 1, c_off + 1);

                // Clear active form state when screen is cleared!
                form_state.active = false;
                form_state.fields.clear();
                form_state.active_idx = 0;
                cur_col = 0;
                cur_row = 0;

                i += 1;
            }
            0x02 => {
                // OP_CRLF inside viewport
                cur_col = 0;
                cur_row += 1;
                if col_offset > 0 {
                    print!("\r\n\x1b[{}C", col_offset);
                } else {
                    print!("\r\n");
                }
                i += 1;
            }
            b'\n' => {
                cur_col = 0;
                cur_row += 1;
                if col_offset > 0 {
                    print!("\r\n\x1b[{}C", col_offset);
                } else {
                    print!("\r\n");
                }
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
                    cur_col = col as u16;
                    cur_row = row as u16;
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
                    if !render_cached_asset(server_node_id, id, col_offset, row_offset) {
                        let _ = req_asset_tx.send(id);
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
            0xC7 => {
                // OP_RENDER_TEMPLATE (asset_id u16, param_count u8, [param_len u8, param_bytes]*)
                if i + 3 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    let param_count = payload[i + 3] as usize;
                    let mut cur = i + 4;
                    let mut params = Vec::new();
                    for _ in 0..param_count {
                        if cur < payload.len() {
                            let p_len = payload[cur] as usize;
                            cur += 1;
                            if cur + p_len <= payload.len() {
                                let s = String::from_utf8_lossy(&payload[cur..cur + p_len]).into_owned();
                                params.push(s);
                                cur += p_len;
                            }
                        }
                    }
                    i = cur;

                    if let Some(template_str) = get_client_asset_content(server_node_id, id) {
                        let expanded = bifrost_bbs::substitute_template(&template_str, &params);
                        print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);
                        let newline_replacement = if col_offset > 0 {
                            format!("\r\n\x1b[{}C", col_offset)
                        } else {
                            "\r\n".to_string()
                        };
                        let aligned_content = expanded
                            .replace("\r\n", "\n")
                            .replace("\n", &newline_replacement);
                        print!("{}", aligned_content);
                    } else {
                        let _ = req_asset_tx.send(id);
                    }
                } else {
                    i += 1;
                }
            }
            0xC8 => {
                // OP_RENDER_MENU (asset_id u16, toggle_mask u32)
                if i + 6 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    let mask = u32::from_be_bytes([payload[i + 3], payload[i + 4], payload[i + 5], payload[i + 6]]);
                    i += 7;

                    if let Some(menu_csv) = get_client_asset_content(server_node_id, id) {
                        let menu_def = bifrost_bbs::parse_menu_csv(&menu_csv);
                        form_state.active = true;
                        form_state.form_id = menu_def.form_id;
                        if let Some(fg) = menu_def.field_fg { form_state.field_fg = fg; }
                        if let Some(bg) = menu_def.field_bg { form_state.field_bg = bg; }
                        let align_mode = menu_def.align.as_deref().unwrap_or("top_left");
                        let is_bottom = align_mode.starts_with("bottom");
                        let is_center = align_mode.ends_with("center") || align_mode == "center";
                        let is_right = align_mode.ends_with("right") || align_mode == "right";

                        let (max_col, term_h_u8) = match layout {
                            LayoutMode::Full => (78u8, 25u8),
                            LayoutMode::Compact => (38u8, 25u8),
                        };

                        // Pre-filter enabled buttons and compute row totals for alignment
                        let mut enabled_buttons = Vec::new();
                        let mut row_widths: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();

                        for (idx, btn) in menu_def.buttons.iter().enumerate() {
                            if idx < 32 && (mask & (1 << idx)) != 0 {
                                let btn_width = (btn.label.len() as u8) + 4;
                                let base_row = if is_bottom {
                                    if btn.row > 0 && btn.row < 10 {
                                        term_h_u8.saturating_sub(btn.row + 1)
                                    } else {
                                        term_h_u8.saturating_sub(3)
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

                            form_state.fields.push(FormField {
                                id: btn.id.clone(),
                                col: field_col,
                                row: field_row,
                                width: btn_width,
                                height: 1,
                                val: btn.label.clone(),
                                is_submit: true,
                                key: btn.key,
                            });
                            apply_color_attribute((form_state.submit_bg << 4) | form_state.submit_fg);
                            print!(
                                "\x1b[{};{}H[ {} ]\x1b[0m",
                                row_offset + field_row as u16 + 1,
                                col_offset + field_col as u16 + 1,
                                btn.label
                            );
                        }
                    } else {
                        let _ = req_asset_tx.send(id);
                    }
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
                                key: None,
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
                            key: None,
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
                                key: None,
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
            0xFD => {
                if i + 1 < payload.len() {
                    let token_id = payload[i + 1] as usize;
                    let client_dict = load_client_dictionary(server_node_id);
                    if let Some(tok_bytes) = client_dict.tokens().get(token_id) {
                        for &b in tok_bytes {
                            if b == b'\r' {
                                cur_col = 0;
                            } else if b == b'\n' {
                                cur_col = 0;
                                cur_row += 1;
                                if col_offset > 0 {
                                    print!("\r\n\x1b[{}C", col_offset);
                                } else {
                                    print!("\r\n");
                                }
                            } else {
                                if cur_col >= max_w {
                                    cur_col = 0;
                                    cur_row += 1;
                                    print!("\x1b[{};{}H", row_offset + cur_row + 1, col_offset + cur_col + 1);
                                }
                                print!("{}", b as char);
                                cur_col += 1;
                            }
                        }
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            c => {
                if c == b'\r' {
                    cur_col = 0;
                } else if c == b'\n' {
                    cur_col = 0;
                    cur_row += 1;
                    if col_offset > 0 {
                        print!("\r\n\x1b[{}C", col_offset);
                    } else {
                        print!("\r\n");
                    }
                } else {
                    if cur_col >= max_w {
                        cur_col = 0;
                        cur_row += 1;
                        print!("\x1b[{};{}H", row_offset + cur_row + 1, col_offset + cur_col + 1);
                    }
                    print!("{}", c as char);
                    cur_col += 1;
                }
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

fn find_workspace_path(relative_path: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(relative_path);
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
        let mut cur = std::path::PathBuf::from(manifest_dir);
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

fn get_node_cache_dir(node_id: &[u8; 32]) -> PathBuf {
    let node_hex: String = node_id.iter().map(|b| format!("{:02x}", b)).collect();
    let base = find_workspace_path(".client_cache");
    base.join(node_hex)
}

fn save_asset_to_cache(node_id: &[u8; 32], asset_id: u16, data: &[u8]) {
    let node_dir = get_node_cache_dir(node_id);
    let _ = std::fs::create_dir_all(&node_dir);
    let ext = if asset_id == 0x00DF {
        "bin"
    } else if data.starts_with(b"# form_id=") || (data.len() > 10 && data[..data.len().min(100)].windows(13).any(|w| w == b"# tag,id,label")) {
        "csv"
    } else if data.windows(2).any(|w| w == b"{{") {
        "tmpl"
    } else {
        "ans"
    };

    let cache_file = if asset_id == 0x00DF {
        node_dir.join("dict.bin")
    } else {
        node_dir.join(format!("{:04x}.{}", asset_id, ext))
    };
    let _ = std::fs::write(&cache_file, data);
}

fn load_client_dictionary(node_id: &[u8; 32]) -> bifrost_compression::CompressionDictionary {
    let node_dir = get_node_cache_dir(node_id);
    let dict_file = node_dir.join("dict.bin");
    if let Ok(bytes) = std::fs::read(&dict_file) {
        if let Ok(dict) = bifrost_compression::CompressionDictionary::from_bytes(&bytes) {
            log::info!(
                "Loaded cached domain dictionary for BBS node (CRC32: 0x{:08X}, {} tokens)",
                dict.crc32(),
                dict.tokens().len()
            );
            return dict;
        }
    }
    // Check config/bbs_dict.bin as pre-seeded dictionary
    let config_dict = find_workspace_path("config/bbs_dict.bin");
    if let Ok(bytes) = std::fs::read(&config_dict) {
        if let Ok(dict) = bifrost_compression::CompressionDictionary::from_bytes(&bytes) {
            return dict;
        }
    }
    bifrost_compression::CompressionDictionary::standard_static()
}

fn get_client_asset_content(node_id: &[u8; 32], asset_id: u16) -> Option<String> {
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
    if let Some((_, rel_path)) = manifest_map.get(&asset_id) {
        let full_path = find_workspace_path(rel_path);
        if let Ok(c) = std::fs::read_to_string(&full_path) {
            return Some(c);
        }
    }

    let node_dir = get_node_cache_dir(node_id);
    let exts = ["csv", "tmpl", "ans", "txt"];
    for ext in &exts {
        let cache_file = node_dir.join(format!("{:04x}.{}", asset_id, ext));
        if let Ok(c) = std::fs::read_to_string(&cache_file) {
            return Some(c);
        }
        let root_file = find_workspace_path(".client_cache").join(format!("{:04x}.{}", asset_id, ext));
        if let Ok(c) = std::fs::read_to_string(&root_file) {
            return Some(c);
        }
    }

    None
}

fn render_cached_asset(
    node_id: &[u8; 32],
    asset_id: u16,
    col_offset: u16,
    row_offset: u16,
) -> bool {
    if let Some(content) = get_client_asset_content(node_id, asset_id) {
        print!("\x1b[{};{}H", row_offset + 1, col_offset + 1);
        let newline_replacement = if col_offset > 0 {
            format!("\r\n\x1b[{}C", col_offset)
        } else {
            "\r\n".to_string()
        };
        let aligned_content = content
            .replace("\r\n", "\n")
            .replace("\n", &newline_replacement);
        print!("{}", aligned_content);
        true
    } else {
        false
    }
}

fn interpret_bytecode_headless(
    server_node_id: &[u8; 32],
    payload: &[u8],
    form_state: &mut FormState,
    req_asset_tx: &tokio::sync::mpsc::UnboundedSender<u16>,
) {
    let mut i = 0;
    while i < payload.len() {
        let op = payload[i];
        match op {
            0x00 | 0x02 | b'\n' | 0x04 => {
                i += 1;
            }
            0x01 => {
                // OP_CLEAR_SCREEN
                form_state.active = false;
                form_state.fields.clear();
                form_state.active_idx = 0;
                i += 1;
            }
            0xC0 => {
                if i + 1 < payload.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            0xC3 => {
                if i + 2 < payload.len() {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            0xC5 => {
                if i + 2 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    let node_dir = get_node_cache_dir(server_node_id);
                    let cache_file = node_dir.join(format!("{:04x}.ans", id));
                    if !cache_file.exists() {
                        let _ = req_asset_tx.send(id);
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
            0xC7 => {
                // OP_RENDER_TEMPLATE
                if i + 3 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    let param_count = payload[i + 3] as usize;
                    let mut cur = i + 4;
                    for _ in 0..param_count {
                        if cur < payload.len() {
                            let p_len = payload[cur] as usize;
                            cur += 1 + p_len;
                        }
                    }
                    i = cur;
                    let node_dir = get_node_cache_dir(server_node_id);
                    let cache_file = node_dir.join(format!("{:04x}.tmpl", id));
                    if !cache_file.exists() {
                        let _ = req_asset_tx.send(id);
                    }
                } else {
                    i += 1;
                }
            }
            0xC8 => {
                // OP_RENDER_MENU
                if i + 6 < payload.len() {
                    let id = u16::from_be_bytes([payload[i + 1], payload[i + 2]]);
                    let mask = u32::from_be_bytes([payload[i + 3], payload[i + 4], payload[i + 5], payload[i + 6]]);
                    i += 7;

                    if let Some(menu_csv) = get_client_asset_content(server_node_id, id) {
                        let menu_def = bifrost_bbs::parse_menu_csv(&menu_csv);
                        form_state.active = true;
                        form_state.form_id = menu_def.form_id;
                        let align_mode = menu_def.align.as_deref().unwrap_or("top_left");
                        let is_bottom = align_mode.starts_with("bottom");
                        let is_center = align_mode.ends_with("center") || align_mode == "center";
                        let is_right = align_mode.ends_with("right") || align_mode == "right";

                        let max_col = 78u8;
                        let term_h_u8 = 25u8;

                        // Pre-filter enabled buttons and compute row totals for alignment
                        let mut enabled_buttons = Vec::new();
                        let mut row_widths: std::collections::HashMap<u8, u8> = std::collections::HashMap::new();

                        for (idx, btn) in menu_def.buttons.iter().enumerate() {
                            if idx < 32 && (mask & (1 << idx)) != 0 {
                                let btn_width = (btn.label.len() as u8) + 4;
                                let base_row = if is_bottom {
                                    if btn.row > 0 && btn.row < 10 {
                                        term_h_u8.saturating_sub(btn.row + 1)
                                    } else {
                                        term_h_u8.saturating_sub(3)
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
                            row_cols.insert(cur_row, cur_col + btn_width + 1);

                            form_state.fields.push(FormField {
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
                    } else {
                        let _ = req_asset_tx.send(id);
                    }
                } else {
                    i += 1;
                }
            }
            0xD0 => {
                // OP_FORM_START
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
                        let id = String::from_utf8_lossy(&payload[i + 5..i + 5 + id_len]).into_owned();
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
                                val,
                                is_submit: false,
                                key: None,
                            });
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
                        let id = String::from_utf8_lossy(&payload[i + 4..i + 4 + id_len]).into_owned();
                        form_state.fields.push(FormField {
                            id: id.clone(),
                            col,
                            row,
                            width: (id.len() + 4) as u8,
                            height: 1,
                            val: String::new(),
                            is_submit: true,
                            key: None,
                        });
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
                i += 1;
            }
            0xD4 => {
                // OP_FORM_FIELD_MULTILINE
                if i + 6 < payload.len() {
                    let col = payload[i + 1];
                    let row = payload[i + 2];
                    let width = payload[i + 3];
                    let height = payload[i + 4];
                    let id_len = payload[i + 5] as usize;
                    if i + 6 + id_len < payload.len() {
                        let id = String::from_utf8_lossy(&payload[i + 6..i + 6 + id_len]).into_owned();
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
                                val,
                                is_submit: false,
                                key: None,
                            });
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
            _ => {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_mode_cycle() {
        assert_eq!(LayoutMode::Full.cycle(), LayoutMode::Compact);
        assert_eq!(LayoutMode::Compact.cycle(), LayoutMode::Full);
    }

    #[test]
    fn test_get_viewport_offsets() {
        let (col, row, w, h) = get_viewport_offsets(LayoutMode::Full, 100, 30);
        assert_eq!(w, 80);
        assert_eq!(h, 25);
        assert_eq!(col, 10);
        assert_eq!(row, 2);

        let (col_c, row_c, w_c, h_c) = get_viewport_offsets(LayoutMode::Compact, 100, 30);
        assert_eq!(w_c, 40);
        assert_eq!(h_c, 25);
        assert_eq!(col_c, 30);
        assert_eq!(row_c, 2);
    }

    #[test]
    fn test_parse_cli_args() {
        let args = vec![
            "bifrost-client".to_string(),
            "--auto-navigate".to_string(),
            "--crawl-steps".to_string(),
            "75".to_string(),
            "--crawl-delay".to_string(),
            "150".to_string(),
            "--headless".to_string(),
            "--log-level".to_string(),
            "debug".to_string(),
            "--addr".to_string(),
            "127.0.0.1:9999".to_string(),
        ];

        let config = parse_cli_args(&args).unwrap().unwrap();
        assert!(config.auto_crawl);
        assert_eq!(config.crawl_steps, 75);
        assert_eq!(config.crawl_delay_ms, 150);
        assert!(config.headless);
        assert_eq!(config.log_level, Some("debug".to_string()));
        assert_eq!(config.server_addr, "127.0.0.1:9999");
    }

    #[test]
    fn test_decide_crawl_action_with_form_buttons() {
        let mut form = FormState {
            active: true,
            form_id: 10,
            fields: vec![
                FormField {
                    id: "nickname".to_string(),
                    col: 2,
                    row: 2,
                    width: 15,
                    height: 1,
                    val: "TestOperator".to_string(),
                    is_submit: false,
                    key: None,
                },
                FormField {
                    id: "read_boards".to_string(),
                    col: 2,
                    row: 5,
                    width: 15,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
                FormField {
                    id: "door_game".to_string(),
                    col: 18,
                    row: 5,
                    width: 15,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
                FormField {
                    id: "logout".to_string(),
                    col: 2,
                    row: 8,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
            ],
            active_idx: 0,
            field_fg: 7,
            field_bg: 0,
            submit_fg: 7,
            submit_bg: 0,
        };

        // Pick action: should choose one of the non-logout submit buttons and preserve nickname
        let action = decide_crawl_action(&mut form, 42);
        assert!(!form.active);
        let parsed: serde_json::Value = serde_json::from_slice(&action.payload).unwrap();
        assert_eq!(parsed["nickname"], "TestOperator");
        let submit_val = parsed["submit"].as_str().unwrap();
        assert!(submit_val == "read_boards" || submit_val == "door_game" || submit_val == "logout");
    }

    #[test]
    fn test_decide_crawl_action_input_only_form() {
        let mut form = FormState {
            active: true,
            form_id: 1,
            fields: vec![FormField {
                id: "nickname".to_string(),
                col: 2,
                row: 2,
                width: 15,
                height: 1,
                val: "Operator".to_string(),
                is_submit: false,
                key: None,
            }],
            active_idx: 0,
            field_fg: 7,
            field_bg: 0,
            submit_fg: 7,
            submit_bg: 0,
        };

        let action = decide_crawl_action(&mut form, 1);
        let parsed: serde_json::Value = serde_json::from_slice(&action.payload).unwrap();
        assert_eq!(parsed["nickname"], "Operator");
    }

    #[test]
    fn test_decide_crawl_action_non_form_keystroke() {
        let mut form = FormState::default();
        let action = decide_crawl_action(&mut form, 7);
        assert_eq!(action.payload.len(), 1);
        assert!(action.description.starts_with("Keystroke"));
    }

    #[test]
    fn test_interpret_bytecode_headless_form_parsing() {
        let server_node = [0xAAu8; 32];
        let mut form = FormState::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Bytecode: OP_CLEAR (0x01), OP_FORM_START (0xD0), OP_FORM_FIELD (0xD1), OP_FORM_SUBMIT (0xD2), OP_FORM_END (0xD3)
        let mut bytecode = Vec::new();
        bytecode.push(0x01); // clear

        // Form Start: form_id=5, field_fg=15, field_bg=1, submit_fg=14, submit_bg=4
        bytecode.extend_from_slice(&[0xD0, 0x05, 15, 1, 14, 4]);

        // Form Field: col=2, row=3, width=10, id_len=4, id="nick", val_len=3, val="Bob"
        bytecode.extend_from_slice(&[0xD1, 2, 3, 10, 4]);
        bytecode.extend_from_slice(b"nick");
        bytecode.push(3);
        bytecode.extend_from_slice(b"Bob");

        // Form Submit: col=2, row=6, id_len=6, id="submit"
        bytecode.extend_from_slice(&[0xD2, 2, 6, 6]);
        bytecode.extend_from_slice(b"submit");

        bytecode.push(0xD3); // Form end

        interpret_bytecode_headless(&server_node, &bytecode, &mut form, &tx);

        assert!(form.active);
        assert_eq!(form.form_id, 5);
        assert_eq!(form.fields.len(), 2);
        assert_eq!(form.fields[0].id, "nick");
        assert_eq!(form.fields[0].val, "Bob");
        assert!(!form.fields[0].is_submit);
        assert_eq!(form.fields[1].id, "submit");
        assert!(form.fields[1].is_submit);
    }

    #[test]
    fn test_crawl_state_explores_multiple_categories_and_avoids_ping_pong() {
        let mut crawler = CrawlState {
            visit_counts: HashMap::new(),
            last_form_id: None,
            last_button_id: None,
            rng_state: 42,
        };

        let make_marketplace_form = || FormState {
            active: true,
            form_id: 50,
            fields: vec![
                FormField {
                    id: "cat_1".to_string(),
                    col: 2,
                    row: 2,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
                FormField {
                    id: "cat_2".to_string(),
                    col: 2,
                    row: 4,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
                FormField {
                    id: "cat_3".to_string(),
                    col: 2,
                    row: 6,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
                FormField {
                    id: "main_menu".to_string(),
                    col: 2,
                    row: 8,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
            ],
            active_idx: 0,
            field_fg: 7,
            field_bg: 0,
            submit_fg: 7,
            submit_bg: 0,
        };

        let make_cat_view_form = || FormState {
            active: true,
            form_id: 52,
            fields: vec![
                FormField {
                    id: "back".to_string(),
                    col: 2,
                    row: 8,
                    width: 10,
                    height: 1,
                    val: String::new(),
                    is_submit: true,
                    key: None,
                },
            ],
            active_idx: 0,
            field_fg: 7,
            field_bg: 0,
            submit_fg: 7,
            submit_bg: 0,
        };

        // Step 1: on marketplace form, picks one category
        let mut form_50 = make_marketplace_form();
        let action1 = crawler.decide_action(&mut form_50);
        let parsed1: serde_json::Value = serde_json::from_slice(&action1.payload).unwrap();
        let cat_first = parsed1["submit"].as_str().unwrap().to_string();

        // Step 2: on category view form, clicks back
        let mut form_52 = make_cat_view_form();
        let action2 = crawler.decide_action(&mut form_52);
        let parsed2: serde_json::Value = serde_json::from_slice(&action2.payload).unwrap();
        assert_eq!(parsed2["submit"], "back");

        // Step 3: back on marketplace form, should pick a DIFFERENT category instead of ping-ponging!
        let mut form_50_second = make_marketplace_form();
        let action3 = crawler.decide_action(&mut form_50_second);
        let parsed3: serde_json::Value = serde_json::from_slice(&action3.payload).unwrap();
        let cat_second = parsed3["submit"].as_str().unwrap().to_string();

        assert_ne!(
            cat_first, cat_second,
            "Crawler should explore different categories instead of repeating {}",
            cat_first
        );
    }

    #[test]
    fn test_render_menu_bytecode_parsing() {
        let server_node = [0x11u8; 32];
        let mut form = FormState::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let menu_asset = get_client_asset_content(&server_node, 0x0103);
        assert!(menu_asset.is_some(), "main_nav.csv asset 0x0103 should resolve");

        let mut bytecode = Vec::new();
        bytecode.push(0x01); // ClearScreen
        bytecode.extend_from_slice(&[0xD0, 10, 15, 4, 14, 1]); // FormStart form_id=10
        bytecode.extend_from_slice(&[0xC8, 0x01, 0x03, 0x00, 0x00, 0x00, 0xFF]); // RenderMenu 0x0103, mask=0xFF
        bytecode.push(0xD3); // FormEnd

        interpret_bytecode(&server_node, &bytecode, &mut form, LayoutMode::Full, 0, 0, &tx);

        assert!(form.active, "Form should be active");
        assert_eq!(form.form_id, 10, "Form ID should be 10");
        assert_eq!(form.fields.len(), 8, "All 8 buttons in main_nav should be populated");
        assert_eq!(form.fields[0].id, "read_boards");
        assert_eq!(form.fields[0].val, "MessageBoards");
        assert_eq!(form.fields[0].key, Some('M'));
        assert!(form.fields[0].is_submit);
    }

    #[test]
    fn test_render_menu_bottom_aligned_stays_in_virtual_terminal() {
        let server_node = [0x11u8; 32];
        let mut form = FormState::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

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
        let (&entry_menu_id, _) = manifest_map
            .iter()
            .find(|(_, (name, _))| name == "voidtrader/entry_menu")
            .expect("voidtrader/entry_menu must be in manifest");

        let mut bytecode = Vec::new();
        bytecode.push(0x01);
        bytecode.extend_from_slice(&[0xD0, 10, 15, 4, 14, 1]);
        bytecode.extend_from_slice(&[
            0xC8,
            (entry_menu_id >> 8) as u8,
            (entry_menu_id & 0xFF) as u8,
            0x00,
            0x00,
            0x00,
            0x1F,
        ]); // 5 buttons mask=0x1F
        bytecode.push(0xD3);

        interpret_bytecode(&server_node, &bytecode, &mut form, LayoutMode::Full, 0, 0, &tx);

        assert!(form.active);
        assert_eq!(form.fields.len(), 5);
        for f in &form.fields {
            // Every field must be strictly within the 1..25 virtual terminal canvas
            assert!(f.row <= 24, "Field row {} should be <= 24 inside virtual terminal", f.row);
            assert!(f.row >= 20, "Bottom aligned field row {} should be >= 20", f.row);
            assert!(f.col + f.width <= 80, "Field col + width {} should not overflow 80 cols", f.col + f.width);
        }
    }
}
