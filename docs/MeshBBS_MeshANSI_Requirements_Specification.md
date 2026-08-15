# System Requirements Document & Protocol Specification: MeshBBS & MeshANSI

**Document Version:** 2.1.0  
**Status:** Architecture & Protocol Specification  
**Language / Core Runtime:** Rust (2021 Edition)  
**Scripting Runtime:** Lua 5.4 Sandboxed Engine (`mlua`)  
**Target Transport:** MeshCore Native Protocol over LoRa PHY (with Mock/Virtual Socket Harness)  
**Target Clients:** Standalone Communicators (LilyGO T-Deck, T-Echo, ESP32-S3, nRF52840), Desktop Terminal Emulators, Companion Mobile Apps  

---

## 1. Executive Summary & Architectural Overview

### 1.1 Objective
The **MeshBBS** project provides an asynchronous, decentralized Bulletin Board System platform engineered specifically for low-power, long-range (LoRa) mesh communication networks using the **MeshCore** protocol. MeshBBS recreates the dial-up BBS era (bulletins, threaded messaging, door games, file catalogs, ANSI art) while addressing the physical limitations of LoRa: extreme bandwidth constraints (250 bps – 5.4 kbps), strict duty-cycle limits, half-duplex contention, and small MTU packet limits.

```
+-------------------------------------------------------------------------------+
|                            MeshBBS Host Engine (Rust)                         |
|  +-------------------------------------------------------------------------+  |
|  | Dynamic Lua Application Framework (Sandboxed `mlua` Runtime)            |  |
|  | - Messaging / Mail  - Door Games  - File Transfer  - Internet Bridging  |  |
|  +-------------------------------------------------------------------------+  |
|  | Session Manager & Virtual Terminal State Machines (Shadow 80x25 / 40x25)|  |
|  +-------------------------------------------------------------------------+  |
|  | MeshANSI Bytecode Compiler & Heatshrink LZSS Streaming Compression      |  |
|  +-------------------------------------------------------------------------+  |
|  | Public Asset Request Handler & Multicast Distributor                    |  |
|  +-------------------------------------------------------------------------+  |
|  | Airtime Budgeting, QoS Priority Queuing & Leaky/Token Bucket Limiter    |  |
|  +-------------------------------------------------------------------------+  |
|  | MeshCore Protocol Multiplexer & Native Cryptographic Identity Engine    |  |
+---------------------------------------+---------------------------------------+
                                        | (Radio Transport Abstraction)
              +-------------------------+-------------------------+
              |                                                   |
              v (Production)                                      v (Testing & CI)
+-------------------------------+               +-------------------------------+
| Native MeshCore LoRa Driver   |               | Virtual Mock Radio Harness    |
| (UART / KISS / Serial / C API)|               | (Local TCP / Socket Broker)   |
| - Duty cycle regulatory guard |               | - Packet loss & latency sim   |
| - Hardware airtime tracking   |               | - Telemetry & metrics tracker |
+---------------+---------------+               +---------------+---------------+
                |                                               |
                | (RF Mesh / Airwaves)                          | (Local Socket)
                v                                               v
+-------------------------------------------------------------------------------+
|                     MeshBBS Client Nodes / Terminal Emulators                 |
|  +-----------------------------------+-------------------------------------+  |
|  | Embedded Hardware (e.g. T-Deck)   | Desktop / Companion App (BLE/Serial)|  |
|  | - MeshCore Decapsulator & Crypto  | - MeshCore Node Peer Daemon         |  |
|  | - Heatshrink LZSS Decompressor    | - CP437 Font Engine / VT100 Canvas  |  |
|  | - MeshANSI Virtual Machine        | - Opportunistic Asset Cache Manager |  |
|  | - Promiscuous Cache Listener (NV) | - Keystroke / Command Batcher       |  |
+--------------------------------------+-------------------------------------+  |
+-------------------------------------------------------------------------------+
```

### 1.2 Core Architectural Principles
1. **Core Platform in Rust:** The host server daemon is built with Rust, leveraging `tokio` for safe, high-concurrency, asynchronous packet processing and zero-cost abstraction over resource constraints.
2. **Modular Lua App Engine:** The Rust core provides the BBS kernel. All end-user applications (Forums, Private Mail, Door Games, File Repositories, Internet/Weather Bridging) are implemented as decoupled Lua scripts running within a sandboxed interpreter.
3. **Dual Transmission Path (Unicast Direct + Public Broadcast):** Private user interactions use authenticated/encrypted direct sessions, while public static assets (logos, UI chrome, door game art) are transmitted via unencrypted public broadcast streams for multi-node opportunistic caching.
4. **Native MeshCore Integration:** Traffic strictly implements native MeshCore packet framing and cryptographic node identity (ED25519 / Curve25519, AES-256) to ensure privacy, integrity, and packet separation from standard chat.
5. **Active Airtime & Channel Regulation:** Built-in token-bucket rate limiting, multicast rate/duty restrictions, and airtime tracking enforce RF duty-cycle compliance and prevent channel exhaustion.
6. **Bandwidth-Optimized MeshANSI Protocol:** Replaces multi-byte ANSI escape codes with a 1-byte opcode binary bytecode and Heatshrink LZSS compression, shrinking 4,000-byte screens down to 200–500 bytes.
7. **Dual-Mode Mock / Production Transport:** Pluggable transport architecture allows complete end-to-end BBS testing, protocol benchmarking, and app development over local TCP/Unix sockets with synthetic channel degradation.

---

## 2. MeshCore Protocol Integration & Cryptography

MeshBBS avoids encapsulation collisions with standard broadcast chat, telemetry, and room routing by strictly integrating with the MeshCore packet architecture.

### 2.1 MeshCore Packet Multiplexing & Channel Separation
MeshCore packets use a 1-byte header byte `0bVVPPPPRR` where:
* `V` (Bits 7–6): Protocol Version (`0b01` for v1).
* `P` (Bits 5–2): Payload Type (4 bits).
* `R` (Bits 1–0): Route Type (`0b00` = Flood / Broadcast, `0b10` = Direct Routing).

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| MeshCore Hdr  | (Transport Codes / Path Routing Context ...)  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| BBS App Port  | Channel Flag  | Frame Seq     | Total Chunks  | <- MeshBBS Sub-Header
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Msg Opcode    | Flags [E/C/Z/B| Payload Len   | CRC16 Checksum|
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Payload: Encrypted Session Bytecode OR Public Broadcast Chunk |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

* **Application Port Identifier:** The first byte of the raw payload is `0xBB` (`MESHBBS_APP_PORT`), preventing non-BBS nodes from attempting to parse bytecode streams as text.
* **Payload Types:**
  * **Unicast Direct Sessions:** Uses `PAYLOAD_TYPE_RAW_CUSTOM` (`0x0F`) or authenticated direct requests (`PAYLOAD_TYPE_REQ` `0x00` / `PAYLOAD_TYPE_RESPONSE` `0x01`). Payload is end-to-end encrypted with the session key.
  * **Public Asset Broadcasts:** Uses `PAYLOAD_TYPE_GRP_DATA` (`0x06`) or unencrypted broadcast flood. Marked with Flag Bit 3 (`B` = Broadcast Asset). Payload is plaintext tokenized bytecode/LZSS.

### 2.2 Identity, Authentication, and Session Security
* **Node Identity:** Users and BBS hosts are identified by their immutable MeshCore Node Public Keys (ED25519/Curve25519) and derived short Node Hashes.
* **Authentication Handshake:**
  1. Client sends a signed connection request containing a random nonce.
  2. BBS verifies the signature against the client's public key and checks the user database.
  3. A temporary ephemeral symmetric session key (AES-256-GCM or ChaCha20-Poly1305) is established via Diffie-Hellman key exchange for unicast sessions.
* **Access Control & Permissions:** Lua applications query the authenticated node identity via `session:get_user_pubkey()` and `session:get_user_callsign()`.

---

## 3. Opportunistic Public Asset Broadcast & Caching Subsystem

To maximize bandwidth efficiency across the entire mesh, static and shared visual assets (BBS Title Logos, Top Headers, UI Borders, Door Game Splash Screens, Bulletin Graphics) are broadcast as public, unencrypted data chunks rather than repeatedly transmitted over point-to-point encrypted sessions.

```
                   +--------------------------------+
                   |  MeshBBS Asset Catalog (Host)  |
                   +---------------+----------------+
                                   |
         +-------------------------+-------------------------+
         | (Targeted Unicast Fallback)                       | (Passive Public Multicast)
         v                                                   v
+-------------------------------+               +-------------------------------+
| Directed Encrypted Stream     |               | On-Demand Public Multicast    |
| - Sent ONLY if client misses  |               | - Triggered by REQ_ASSET      |
|   the asset after querying    |               | - Unencrypted public frames   |
+---------------+---------------+               +---------------+---------------+
                |                                               |
                | (Unicast to Client A)                         | (Omnidirectional RF Broadcast)
                v                                               v
+-------------------------------+               +-------------------------------+
| Client Node A (Interactive)   |               | ALL Mesh Nodes in RF Range    |
| - Queries flash cache         |               | - Client A, Client B, Node C  |
| - Renders instantly if cached |               | - Promiscuously listen & chunk|
| - Emits REQ_ASSET if missing  |               | - Reassemble & verify CRC32   |
|                               |               | - Store in local Flash/NVS    |
+-------------------------------+               +-------------------------------+
```

### 3.1 Passive Multicast Sync Protocol
1. **Content Addressing:** Every public asset is indexed by an immutable 32-bit CRC or 16-bit short Content ID (`AssetID`).
2. **On-Demand Public Multicast:** The host only transmits public assets when a connected client reports a cache miss (via `REQ_ASSET`). By broadcasting the asset publicly, all other listening nodes in RF range can passively intercept, capture, and cache the chunks simultaneously.

### 3.2 Promiscuous Client Cache Assembly
Client nodes (even when not logged into the BBS) run a lightweight background listener:
* **Chunk Header:** Broadcast asset packets contain `AssetID`, `ChunkIndex`, `TotalChunks`, and `PayloadCRC16`.
* **SRAM Assembly Scratchpad:** Incoming chunks are held in a small temporary bitmap buffer (typically 1–2 KB).
* **Non-Volatile Persistence:** Once all chunks for an `AssetID` are received and verified against the master CRC32, the assembled bytecode is committed to local SPI flash / LittleFS storage.
* **Instant Rendering:** When any client later logs in and receives the 3-byte command `[OP_RENDER_ASSET] [AssetID]`, the screen is painted instantly from local storage with **zero incremental radio airtime**.

### 3.3 Broadcast Asset Packet Structure

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| AppPort (0xBB)| MsgType (0x04)| Flags [B=1]   | Chunk Index   |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Total Chunks  | Asset ID (High)| Asset ID (Low)| Payload Length|
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Asset Master CRC32 (4 Bytes)                                  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Chunk Payload (Compressed MeshANSI Bytecode) ...              |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

---

## 4. Configurable Packet Rate Limiting & Airtime Regulator

To prevent channel saturation and comply with ISM duty cycles, MeshBBS implements a multi-tier **Airtime Budgeting & Priority Queue** subsystem in Rust.

```
       [Lua Apps / UI Engine]             [Passive Asset Broadcasts]
                   |                                  |
                   | (Unicast Interactivity)          | (On-Demand Multicast Art)
                   v                                  v
     +------------------------------------------------------+
     |             5-Tier QoS Priority Queue                |
     | Priority 0: Session ACKs & Handshake Handlers        |
     | Priority 1: Interactive Keystroke Feedback & Deltas  |
     | Priority 2: Full Screen Bytecode Streams             |
     | Priority 3: Bulk Data (Private Mail / File Chunks)   |
     | Priority 4: On-Demand Public Asset Broadcasts        |
     +--------------------------+---------------------------+
                                |
                                v
     +------------------------------------------------------+
     |          Token-Bucket Packet Rate Limiter            |
     |          - Limits instantaneous burst & pkts/min     |
     +--------------------------+---------------------------+
                                |
                                v
     +------------------------------------------------------+
     |            Airtime Regulator (Duty Cycle)            |
     |          - Enforces regional max duty cycle %        |
     +--------------------------+---------------------------+
                                |
                                v
     +------------------------------------------------------+
     |         Hardware Radio Driver / Transmit Buffer      |
     +------------------------------------------------------+
```

### 4.1 Rate Limiter Configuration Schema (`config.toml`)

```toml
[rate_limiter]
# Maximum packets per minute across all active BBS sessions
max_packets_per_minute = 45

# Maximum instantaneous burst size before hard throttling kicks in
max_burst_packets = 4

# Minimum quiet period between consecutive packet transmissions (milliseconds)
inter_packet_guard_ms = 350

# Maximum allowed transmitter duty cycle percentage (e.g., 1.0% for EU868)
max_duty_cycle_percent = 1.0

# Rolling window for airtime duty cycle calculation (seconds)
duty_cycle_window_secs = 3600

# Channel congestion backoff multiplier when collision/retry is detected
congestion_backoff_factor = 1.5

[asset_broadcaster]
# Enable passive public broadcasting of assets upon client request (multicast cache population)
enable_on_demand_broadcast = true

# Maximum duty cycle allocated exclusively to public asset broadcasts (%)
max_asset_broadcast_duty_cycle = 0.15
```

### 4.2 Airtime Calculation & Transmission Budgeting
The server calculates expected airtime ($T_{	ext{air}}$) for every outbound packet based on current LoRa radio parameters:

$$T_{	ext{air}} = T_{	ext{preamble}} + T_{	ext{payload}}$$

$$T_{	ext{payload}} = N_{	ext{symbols}} 	imes T_{	ext{sym}} \quad 	ext{where} \quad T_{	ext{sym}} = rac{2^{	ext{SF}}}{	ext{BW}}$$

* If the rolling duty cycle exceeds `max_duty_cycle_percent`, low-priority queues (Priority 3 & 4) are stalled.
* Interactive sessions receive priority tokens; the public asset multicast strictly yields to live user interactions.

---

## 5. Lua Modular Application Framework

The BBS core acts as a microkernel. All user-facing features are decoupled into standalone **Lua Applications** executed in a memory-bounded, sandboxed environment via `mlua`.

```
/meshbbs/
  ├── config.toml
  ├── assets/                # Static ANSI screens, icons, bitmaps
  │   ├── manifest.toml      # Catalog mapping names to 16-bit AssetIDs
  │   └── *.ans
  └── apps/
      ├── 00_main_menu.lua   # System entrypoint and top-level navigation
      ├── 10_messages.lua    # Public threaded message boards
      ├── 20_mail.lua        # End-to-end encrypted private mail
      ├── 30_doorgames/      # Turn-based Door games
      │   ├── tradewars.lua
      │   └── lord.lua
      ├── 40_filegate.lua    # Chunked file repository & OTA bulletin sync
      └── 50_bridges/        # Read-only external bridges
          └── weather.lua    # Cached off-grid weather distributor
```

### 5.1 Lua Sandboxing & Resource Isolation
* **Instruction Counter Limit:** Max 500,000 instructions per execution slice to prevent infinite loops.
* **Memory Quota:** Each Lua VM is bounded to a maximum of 512 KB heap RAM.
* **Restricted Standard Libraries:** Direct OS access, raw filesystem I/O, and raw sockets are stripped. All external operations are mediated by safe Rust Host APIs.

### 5.2 Rust-to-Lua Host API Specification

#### Screen & Terminal API (`term`)
* `term.clear()`: Clears client terminal and resets cursor.
* `term.move_to(col, row)`: Positions cursor at `(col, row)`.
* `term.set_color(fg, bg)`: Sets active 16-color CGA/EGA palette.
* `term.print(text)`: Emits printable text string.
* `term.draw_box(col, row, width, height, border_style)`: Emits optimized CP437 box macro.
* `term.render_asset(asset_name_or_id)`: Emits opcode to render a pre-cached client asset.
* `term.flush()`: Compiles terminal mutations into MeshANSI delta bytecode and queues for transmit.

#### Session & Identity API (`session`)
* `session.node_id()`: Returns hex string of client's MeshCore public key.
* `session.callsign()`: Returns registered nickname/callsign.
* `session.await_input(max_len, callback)`: Prompts client for input and suspends execution until response arrives.
* `session.close()`: Gracefully terminates connection.

#### Persistent Storage API (`db`)
* `db.get(table, key)`: Retrieves key-value record from embedded database (e.g., Sled / SQLite).
* `db.set(table, key, value)`: Stores string/JSON document.
* `db.query(table, prefix, limit)`: Iterates records for message threads or scoreboards.

#### Sample Lua App: Turn-Based Door Game (`minidungeon.lua`)

```lua
-- Simple Turn-Based Combat Door Game for MeshBBS
local app = {}

function app.on_start(session)
    term.clear()
    -- Renders public cached banner (0 airtime if pre-cached via broadcast)
    term.render_asset("ASSET_DUNGEON_BANNER")
    term.move_to(2, 8)
    term.set_color(14, 0) -- Yellow on Black
    term.print("=== THE LORA CATACOMBS ===")
    
    local player = db.get("dungeon_players", session.node_id()) or { hp = 20, gold = 0, level = 1 }
    term.move_to(2, 10)
    term.set_color(7, 0)
    term.print(string.format("Hero: %s | HP: %d/20 | Gold: %d", session.callsign(), player.hp, player.gold))
    
    term.move_to(2, 12)
    term.print("[1] Explore Crypt  [2] Rest at Camp  [Q] Exit")
    term.flush()
    
    session.await_input(1, function(input)
        if input == "1" then
            app.battle(session, player)
        elseif input == "2" then
            player.hp = 20
            db.set("dungeon_players", session.node_id(), player)
            term.move_to(2, 14)
            term.set_color(10, 0) -- Green
            term.print("You rested and restored your HP! Press any key...")
            term.flush()
            session.await_input(1, function() app.on_start(session) end)
        else
            session.load_app("00_main_menu")
        end
    end)
end

function app.battle(session, player)
    local monster_hp = math.random(5, 12)
    term.move_to(2, 14)
    term.set_color(12, 0) -- Light Red
    term.print(string.format("A Wild Mesh Goblin appears! (HP: %d)", monster_hp))
    player.gold = player.gold + 5
    player.hp = math.max(1, player.hp - 3)
    db.set("dungeon_players", session.node_id(), player)
    term.move_to(2, 16)
    term.set_color(11, 0) -- Cyan
    term.print("You defeated the Goblin and earned 5 Gold!")
    term.flush()
    session.await_input(1, function() app.on_start(session) end)
end

return app
```

---

## 6. MeshANSI Wire Protocol & Compression Specification

MeshANSI converts standard terminal visual operations into a high-density 1-byte opcode command stream.

### 6.1 Opcode Specification

| Opcode Range | Identifier | Operands | Function |
|---|---|---|---|
| `0x00` | `OP_NOP` | None | No operation. |
| `0x01` | `OP_CLEAR_SCREEN` | None | Clear 80x25 canvas, cursor to (0,0), reset attributes. |
| `0x02` | `OP_CRLF` | None | Advance to start of next line. |
| `0x03` | `OP_PAGE_PAUSE` | None | Display `[Press Key]` and wait. |
| `0x04` | `OP_END_OF_FRAME` | None | Terminal redraw commit point. |
| `0x20 - 0x7E` | `LITERAL_ASCII` | None | Direct 7-bit standard printable ASCII. |
| `0x80 - 0xBF` | `EXT_CP437_DIRECT` | None | Top 64 CP437 box-drawing & shading characters. |
| `0xC0` | `OP_SET_COLOR` | `Attr (1B)` | High Nibble = BG (0–15), Low Nibble = FG (0–15). |
| `0xC1` | `OP_RLE_GLYPH` | `Count (1B), Glyph (1B)` | Run-Length Repeat Character. |
| `0xC2` | `OP_RLE_SPACE` | `Count (1B)` | Run-Length Skip Blank Spaces. |
| `0xC3` | `OP_CURSOR_ABS` | `Col (1B), Row (1B)` | Jump to absolute screen coordinate. |
| `0xC4` | `OP_CURSOR_REL` | `dCol (1B), dRow (1B)` | Relative offset movement. |
| `0xC5` | `OP_RENDER_ASSET` | `AssetID (2B or 4B)` | Render static screen from local client cache. |
| `0xC6` | `OP_DELTA_BLOCK` | `Col, Row, W, H` | Bounding box header for differential update. |
| `0xFE` | `OP_RAW_CP437` | `Byte (1B)` | Unmapped 8-bit CP437 raw character escape. |

### 6.2 Compression Pipeline
1. **ANSI CSI Tokenization:** Eliminates variable-length escape sequences (`[31;42m` $ightarrow$ `0xC0 0x24`).
2. **Delta Extraction:** Computes difference matrix between previous and current screen states. Unchanged cells are skipped via `OP_CURSOR_ABS`.
3. **Heatshrink LZSS Encoding:** Tokenized bytecode is passed through a Heatshrink compressor configured with an 8-bit window ($2^8 = 256	ext{ bytes}$) and a 4-bit lookahead ($2^4 = 16	ext{ bytes}$).

---

## 7. Mock / Virtual Radio Testing & Telemetry Layer

To enable rapid development without transmitting on live ISM airwaves, the Rust architecture incorporates a decoupled transport abstraction layer.

```
                  +--------------------------------+
                  |    Rust RadioTransport Trait   |
                  +---------------+----------------+
                                  |
            +---------------------+---------------------+
            |                                           |
            v                                           v
+-------------------------------+       +-------------------------------+
| Hardware Radio Driver         |       | Virtual Socket Radio Harness  |
| - Serial / KISS / SPI SX1262  |       | - TCP / Unix Domain Socket    |
| - Hardware RSSI / SNR feedback|       | - Synthetic Channel Impairment|
+-------------------------------+       +---------------+---------------+
                                                        |
                                                        v
                                        +-------------------------------+
                                        | Telemetry & Metrics Engine    |
                                        | - Virtual Airtime Tracking    |
                                        | - Bytecode Compression Ratios |
                                        | - Cache Hit/Miss Bandwidth Log|
                                        +-------------------------------+
```

### 7.1 Rust Transport Trait Interface

```rust
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct RadioPacket {
    pub is_broadcast: bool,
    pub src_node: [u8; 32],
    pub dst_node: [u8; 32],
    pub payload: Vec<u8>,
    pub signal_rssi: i16,
    pub signal_snr: i8,
}

#[async_trait]
pub trait RadioTransport: Send + Sync {
    async fn send_packet(&self, packet: RadioPacket) -> Result<(), TransportError>;
    async fn receive_packet(&self) -> Result<RadioPacket, TransportError>;
    fn get_estimated_airtime_ms(&self, payload_len: usize) -> u32;
    fn get_current_duty_cycle(&self) -> f32;
}
```

### 7.2 Virtual Radio Simulation & Telemetry Metrics
The Virtual Mock Transport enables automated integration tests and logs key protocol metrics:
* **Configurable Synthetic Impairments:**
  * `packet_loss_rate`: Simulates dropped packets (e.g., 0.05 for 5% drop rate).
  * `simulated_latency_ms`: Injects realistic LoRa time-on-air delays.
  * `max_packet_mtu`: Enforces strict payload fragmentation boundaries (e.g., 200 bytes).
* **Telemetry Reporting:**
  * **Payload Compression Efficiency:** Compares raw ANSI bytecount against transmitted compressed bytecode.
  * **Cache Hit vs. Airtime Savings:** Tracks the exact RF airtime saved across multiple simulated clients due to public asset pre-caching.
  * **Channel Saturation Alerts:** Triggers warnings when rate-limiter backoff queues exceed defined thresholds.

---

## 8. Functional & Non-Functional Requirements

### 8.1 Functional Requirements Matrix

| ID | Category | Requirement Description |
|---|---|---|
| **FR-CORE-01** | Core Architecture | The BBS host server daemon must be implemented in Rust using async `tokio`. |
| **FR-CORE-02** | App Runtime | The core must provide an isolated Lua 5.4 runtime environment executing sandboxed app scripts. |
| **FR-CORE-03** | MeshCore Protocol | The transport framing must implement MeshCore packet formats and use `0xBB` application multiplexing. |
| **FR-CORE-04** | Crypto / Auth | Direct sessions must be authenticated and encrypted using native MeshCore asymmetric node keys. |
| **FR-BCAST-01** | Asset Broadcast | Public assets must be broadcast unencrypted as group/flood frames on a configurable trickle schedule. |
| **FR-BCAST-02** | Promiscuous Cache | Client firmware must promiscuously capture, assemble, verify, and store public broadcast assets in flash. |
| **FR-RATE-01** | Rate Limiting | The host must enforce a configurable token-bucket packet limiter and airtime duty-cycle regulator. |
| **FR-RATE-02** | QoS Queuing | Transmit packets must be scheduled via a 5-tier priority queue (ACK > Delta > Screen > Bulk > Broadcast). |
| **FR-ANSI-01** | MeshANSI | The host and client must encode and decode the MeshANSI 1-byte opcode bytecode specification. |
| **FR-ANSI-02** | Compression | Bytecode streams must be compressed using the Heatshrink LZSS algorithm ($W=8, L=4$). |
| **FR-MOCK-01** | Test Harness | The platform must provide a mock socket radio transport for hardware-free local testing and metrics logging. |

### 8.2 Non-Functional & Performance Targets

| Metric | Target Specification | Design Constraint |
|---|---|---|
| **Compressed Screen Payload** | `< 450 Bytes` (Average 80x25 Screen) | Fits within 2 to 3 LoRa MTU packets. |
| **Cached Asset Airtime** | `0 ms` Incremental Airtime (Cache Hit) | Requires only 3-byte `OP_RENDER_ASSET` command. |
| **Interactive Delta Update** | `< 64 Bytes` (Single Packet) | Instant response for menu and form navigation. |
| **Host Memory Usage** | `< 64 MB RAM` (Server with 50 active sessions) | Capable of running on small SBCs (Raspberry Pi Zero 2W). |
| **Client Memory Footprint** | `< 8 KB RAM` Total BBS Buffer Allocation | Compatible with resource-constrained MCUs (nRF52840, ESP32). |
| **Lua Script Execution Budget** | `< 500,000` Virtual Instructions per Event | Prevents CPU exhaustion from malicious or bugged scripts. |
| **Duty Cycle Compliance** | Guaranteed $\le 1.0\%$ (Configurable by Region) | Conforms to legal ISM RF operational requirements. |

---

## 9. Implementation Roadmap

* **Phase 1: Rust Core & Transport Abstraction**
  * Implement the Rust `RadioTransport` trait and the Virtual Socket Mock Harness.
  * Build the token-bucket rate limiter and airtime duty-cycle regulator.
  * Integrate native MeshCore packet framing and header decapsulation.
* **Phase 2: MeshANSI Compiler & Broadcast Asset Engine**
  * Develop the `meshansi` Rust crate (ANSI parser, opcode generator, Heatshrink compression).
  * Build the passive on-demand Public Multicast asset distributor.
  * Build embedded C/C++ reference decoder and promiscuous cache manager for client nodes.
* **Phase 3: Lua Application Runtime**
  * Integrate `mlua` sandbox into the Rust engine.
  * Expose `term`, `session`, `db`, and `net` Host APIs to Lua.
  * Implement core system applications (`00_main_menu.lua`, `10_messages.lua`, `30_doorgames/minidungeon.lua`).
* **Phase 4: Field Testing & MeshCore Radio Deployment**
  * Connect Rust core to physical LoRa transceivers via UART/KISS/SPI drivers.
  * Validate multi-hop routing, opportunistic caching across multiple field devices, and RF channel performance.
