# Product Requirement Document (PRD): MeshBBS & MeshANSI

**Document Version:** 1.0.0  
**Target Release:** v1.0-alpha  
**Author:** Antigravity (AI Coding Assistant)  
**Status:** Approved for Development  

---

## 1. Executive Summary & Product Vision

### 1.1 Problem Statement
Standard off-grid communications, especially over low-power, long-range (LoRa) radio networks, are currently limited to simple, unstructured text-based messaging. While highly functional for point-to-point alerts, they lack the structural community-building tools—such as threaded message boards, persistent file repositories, off-grid database access, and interactive group activities—that dial-up Bulletin Board Systems (BBS) once provided.

Recreating these services over LoRa presents severe engineering challenges:
1. **Severe Bandwidth Limitations:** LoRa data rates range from 250 bps to 5.4 kbps, making standard telnet, SSH, or ANSI escape sequences (which are verbose and multi-byte) unusable.
2. **Duty-Cycle Restrictions:** Strict legal regulations (such as EU868's 1.0% duty cycle limit) restrict continuous transmission, requiring active airtime budgeting.
3. **Half-Duplex Contention:** High collision rates when multiple nodes transmit simultaneously.
4. **Hardware Constraints:** Client communicators (e.g., LilyGO T-Deck, T-Echo) run on low-power microcontrollers (ESP32-S3, nRF52840) with limited RAM (typically <8 KB for terminal buffering) and flash memory.

### 1.2 Product Vision
**MeshBBS** is a decentralized, asynchronous Bulletin Board System platform designed specifically for long-range, off-grid LoRa networks utilizing the **MeshCore** protocol. It provides a modular, interactive environment (forums, private mail, file gates, door games, and local bridges) while keeping radio transmission footprint minimal through a custom binary terminal bytecode (**MeshANSI**) and an opportunistic client-side caching architecture.

The vision is to establish an off-grid digital community center that runs on solar/battery-powered host nodes, serving an entire valley, neighborhood, or emergency zone without internet dependencies.

---

## 2. Key Target Personas & Use Cases

### 2.1 Personas
* **The Off-Grid Coordinator (Host Operator):** Runs a local MeshBBS node from a high-altitude repeater. Needs a highly stable, low-maintenance host daemon that respects regional RF regulations and allows quick extension of local services (weather, emergency notices, bulletins) without rebooting the server.
* **The Emergency Responder (Client):** Uses a portable communicator (e.g., T-Deck) to access central bulletins, exchange encrypted mail, or retrieve weather maps off-grid. Needs immediate, low-latency display of UI components and structured navigation.
* **The Hobbyist Mesh Operator (Client):** Enjoys playing asynchronous door games, checking message boards, and uploading/downloading small telemetry files. Needs a retro-themed, highly interactive experience that doesn't exhaust the battery or hog the local channel.

### 2.2 Core Use Cases
* **UC-01: Accessing Public Bulletins & Forums:** A user logs in and browses the main menu, navigates to the "Forums" board, and reads the latest messages.
* **UC-02: Sending Encrypted Private Mail:** A user sends an authenticated, end-to-end encrypted message to another node hash.
* **UC-03: Asynchronous Door Games:** A user plays a turn-based game (e.g., *Lore Catacombs*), with combat status and score boards saved in the database.
* **UC-04: Off-Grid Weather/Data Bridge:** A user checks local sensor telemetry or weather forecasts bridged from the host node's sensors or internet connection.
* **UC-05: Silent Background Asset Sync:** A client communicator is turned on and left in a pocket. It automatically sniff-gathers fragments of UI borders, graphics, and logos broadcast by the BBS. When the user later connects to the BBS, all UI elements render instantly with zero airtime cost.

---

## 3. Detailed Product Features & Scope

### 3.1 Core BBS Host Engine (Rust)
* **Description:** A highly concurrent, asynchronous server daemon written in Rust, leveraging `tokio`. It serves as the BBS microkernel.
* **Functional Requirements:**
  * **Session Manager:** Tracks active connections, binds them to virtual terminal state machines, and manages input-response loops.
  * **Virtual Terminals:** Maintains a shadow representation of the client terminal (80x25 or 40x25 characters) to calculate screen updates and delta differences.
  * **Database Adapter:** Embeds a local persistent database (e.g., Sled or SQLite) to store messages, user credentials, game saves, and file metadata.

### 3.2 MeshCore Protocol Integration & Cryptography
* **Description:** Connection, session authentication, and data transmission must strictly adhere to the MeshCore packet architecture.
* **Functional Requirements:**
  * **Multiplexing:** Use application port identifier `0xBB` (`MESHBBS_APP_PORT`) in the payload header to isolate BBS traffic.
  * **Cryptographic Identity:** Map user sessions to their MeshCore ED25519 public keys. No plain-text passwords; authentication is validated via signed challenge-response handshakes.
  * **Session Encryption:** Establish ephemeral symmetric keys (AES-256-GCM / ChaCha20-Poly1305) via Diffie-Hellman to encrypt all unicast session traffic.

### 3.3 Passive Multicast Asset Caching & Per-Node Storage
* **Description:** Public static assets (ANSI screens, graphics, menus) and live trained domain dictionaries (Asset `0x00DF`) are broadcast as unencrypted public frames only when requested by an active client, allowing all nearby listening nodes to passively capture and cache them in local storage.
* **Functional Requirements:**
  * **On-Demand Public Multicast:** When a connected client requests a missing asset via `REQ_ASSET`, the server broadcasts the asset chunks publicly as group frames, allowing all nodes in RF range to receive and cache it.
  * **Per-Node Cache Partitioning:** Clients partition stored assets by BBS Node ID (`.client_cache/<bbs_node_hex>/`), enabling seamless multi-BBS roaming without asset collision.
  * **Promiscuous Cache Listener:** Client devices listen to public asset broadcasts, reassemble them, verify their integrity using CRC32, and save them to local storage (SPI Flash / LittleFS).
  * **Cache Synchronization:** The BBS server invokes cached assets via the `OP_RENDER_ASSET` opcode. If a client lacks an asset, it issues a `REQ_ASSET` packet, which prompts the server to broadcast it.

### 3.4 Rate Limiting & Airtime Regulator
* **Description:** Active duty-cycle enforcement and packet pacing to maintain compliance with legal limits and protect channel capacity.
* **Functional Requirements:**
  * **Token Bucket Limiter:** Restricts peak packet rate and instantaneous burst sizes.
  * **Rolling Airtime Window:** Tracks transmitter time-on-air and delays low-priority packets if regional limits (e.g., 1.0%) are exceeded.
  * **5-Tier Quality of Service (QoS):**
    * **Priority 0:** Session ACKs & Handshakes (Critical).
    * **Priority 1:** Interactive Keystroke Feedback & Cursor Deltas.
    * **Priority 2:** Full-Screen Bytecode Streams.
    * **Priority 3:** Bulk Data (Private Mail, File Gate chunks).
    * **Priority 4:** On-Demand Public Asset Broadcasts.

### 3.5 MeshANSI Bytecode & Multi-Tier Adaptive Compression Engine
* **Description:** A bandwidth-efficient replacement for ANSI escape codes that tokenizes layout operations and compresses data through an adaptive multi-tier pipeline.
* **Functional Requirements:**
  * **Bytecode Tokenizer:** Replaces long CSI sequences (e.g., `\x1b[31;42m`) with compact 1-byte opcodes (e.g., `0xC0 0x24` for red on green).
  * **Run-Length Encoding (RLE):** Implements `OP_RLE_GLYPH` and `OP_RLE_SPACE` to collapse repetitive text and background blocks.
  * **Screen Delta Extractor:** Compares the updated shadow terminal with the previous state and transmits only changed cells, jumping to coordinates with `OP_CURSOR_ABS`.
  * **Domain-Specific Dictionary Encoding:** Replaces high-frequency UI strings, form labels, box lines, and opcode patterns with 1-byte token references (`0xFD <idx>`). Supports dynamic 254-token domain dictionaries broadcast via Asset `0x00DF`.
  * **Heatshrink LZSS Compression:** Compresses bytecode streams using sliding-window LZSS ($W=6..8, L=4..5$).
  * **Compound Compression:** Automatically chains domain dictionary tokenization with Heatshrink LZSS for up to **+36.9% net airtime reduction**.
  * **Anti-Expansion Guard:** Automatically compares compressed payload against raw byte count; falls back to uncompressed raw transmission (`flags = 0x00`) if compression yields zero or negative savings.

### 3.6 Session-Level Packet Deduplication (Hash-Referencing)
* **Description:** Eliminates redundant over-the-air transmissions when clients repeatedly navigate back and forth between identical menus or screens.
* **Functional Requirements:**
  * **Session Payload Ring Buffer:** Both server and client maintain an LRU cache (100 entries) of recently transmitted/received screen frames keyed by 32-bit CRC.
  * **Hash-Only Packets:** If a screen payload matches an existing entry in the session cache, the server sets Sub-Header Flag `0x08` (`SESSION_DEDUP_HASH`) and transmits only the 4-byte CRC32 integer, achieving **90–98% airtime savings** for recurring views.
  * **NACK Recovery Protocol:** If a client receives a hash reference for an un-cached frame, it transmits a NACK message (`Opcode 0x06`) requesting immediate full payload retransmission.

### 3.7 10-Minute Seamless Session Resumption
* **Description:** Preserves user workflow across brief RF dropouts, deep sleep cycles, or client terminal reconnects.
* **Functional Requirements:**
  * **10-Minute Timeout Window (`SESSION_RESUME_TIMEOUT_SECS = 600`):** Reconnections within 10 minutes reuse the active session state and invoke the `on_resume()` Lua hook to redraw the active viewport without restarting the session.

### 3.8 Lua Sandboxed Application Runtime
* **Description:** Safe, sandboxed runtime in the Host server where forums, games, and bridges are executed dynamically without compromising the main daemon.
* **Functional Requirements:**
  * **Lua Sandbox (`mlua`):** Restricts access to standard libraries (no `os`, `io`, `debug`, or raw `socket`). Binds memory usage to $\le 512$ KB and execution to $\le 500,000$ instructions per cycle.
  * **Rust Host APIs:** Exposes core subsystems via safe global bindings:
    * `term`: Controls printing, colors, absolute positioning, box macros, asset rendering, and screen flushes.
    * `session`: Exposes node info and prompts for input asynchronously.
    * `db`: Provides key-value persistent storage.
  * **Application Loading:** Dynamically executes encapsulated applications from `/bifrost/apps/<app_id>/` (each with `manifest.toml`, `main.lua`, and local `assets/`) configured via `config.toml`.
  * **Dynamic Asset Registration:** Automatically discovers asset declarations in `manifest.toml`, allocates collision-free 16-bit AssetIDs dynamically, and provides relative (`"banner"`) and namespaced (`"main_menu/banner"`) asset resolution.

### 3.9 Diagnostics, Automated Crawling & Tuning Suite (`bifrost-tuning`)
* **Description:** Tooling for continuous diagnostic capture, automated traffic simulation, and data-driven compression parameter tuning.
* **Functional Requirements:**
  * **Server Packet Capture:** CLI flag `--capture [DIR]` captures raw and compressed payloads with per-session CSV logging and clean initialization.
  * **Automated Client Crawler:** Simulated navigation agent with inverse-visit weighted branch exploration (`--crawl`, `--headless`, `--crawl-steps`, `--crawl-delay`) to fuzz menus and generate realistic training corpora.
  * **Tuning CLI (`bifrost-tuning`):** Dedicated tool to run compression benchmarks (`analyze`), train custom 254-token dictionaries (`train`), and perform parameter grid searches (`sweep`).

---

## 4. Non-Functional & Quality Requirements

### 4.1 Performance Targets
* **Bandwidth Optimization:** An average 80x25 screen must compress to less than 450 bytes (fitting inside 2–3 LoRa MTUs).
* **Zero-Airtime UI Rendering:** Cached UI assets must render instantly from local flash on the client upon receiving a 3-byte `OP_RENDER_ASSET` code.
* **Host Resource Bounds:** The host server must run within 64 MB RAM and consume minimal CPU, allowing deployment on low-cost single-board computers (e.g., Raspberry Pi Zero 2W).
* **Client Memory Footprint:** The client MeshANSI VM and decompression buffer must require less than 8 KB of SRAM.

### 4.2 Reliability & Error Handling
* **Graceful Degradation:** If packet loss is high, the session must retransmit unacknowledged packets without dropping the session state.
* **Duty Cycle Enforcement:** The regulator must guarantee compliance with the regional duty-cycle limit (configurable, default 1.0%), queuing non-urgent traffic indefinitely if needed.
* **Sandboxed Fault Isolation:** A crashed or infinite-looping Lua script must terminate gracefully with an error message shown to the user, returning them to the main menu without crashing the Rust core.

### 4.3 Portability & Extensibility
* **Pluggable Transport:** The core must easily switch between the hardware radio interface (UART/KISS/SPI) and the virtual test harness.
* **Cross-Platform Host:** The Rust engine must compile on Linux (x86_64, AArch64) and macOS.
* **Client Code Compatibility:** The MeshANSI decoder and Heatshrink decompressor must compile on ESP-IDF (C/C++), Arduino, and desktop rust clients.

---

## 5. System Architecture & Information Flows

### 5.1 System Component Block Diagram
```
                     +---------------------------------------+
                     |         MeshBBS Host Daemon           |
                     |  +---------------------------------+  |
                     |  |         Tokio Core Event Loop   |  |
                     |  |  +---------------------------+  |  |
                     |  |  |    Lua Sandboxed Runtime  |  |  |
                     |  |  |    (mlua, apps/, Sled DB) |  |  |
                     |  |  +-------------+-------------+  |  |
                     |  |                |               |  |
                     |  |  +-------------v-------------+  |  |
                     |  |  | Virtual Terminal State &  |  |  |
                     |  |  | MeshANSI Bytecode Compiler|  |  |
                     |  |  +-------------+-------------+  |  |
                     |  |                |               |  |
                     |  |  +-------------v-------------+  |  |
                     |  |  |    Rate Limiter & QoS     |  |  |
                     |  |  |    Priority Queues        |  |  |
                     |  |  +-------------+-------------+  |  |
                     |  +----------------+----------------+  |
                     +-------------------|-------------------+
                                         | (RadioTransport Trait)
                      +------------------+------------------+
                      |                                     |
                      v (Production)                        v (Testing/Mock)
          +-----------+-----------+             +-----------+-----------+
          | Hardware KISS/UART    |             | Local TCP Socket      |
          | LoRa Radio Driver     |             | Simulation Harness    |
          +-----------+-----------+             +-----------+-----------+
                      |                                     |
                      v (LoRa RF Medium)                    v (TCP Loopback)
          +-----------+-------------------------------------+-----------+
          |                  MeshBBS Client Terminal                    |
          |  +--------------------+  +-------------------------------+  |
          |  | MeshCore Decryptor |  | Promiscuous Broadcast Listener|  |
          |  +----------+---------+  +---------------+---------------+  |
          |             |                            |                  |
          |  +----------v---------+                  |                  |
          |  | Heatshrink Decoded |                  |                  |
          |  +----------+---------+                  |                  |
          |             |                            |                  |
          |  +----------v---------+  +---------------v---------------+  |
          |  |  MeshANSI Engine   <--+  SPI Flash Asset Cache (FS)   |  |
          |  +----------+---------+  +-------------------------------+  |
          |             |                                               |
          |  +----------v---------+                                     |
          |  | CP437 Screen Canvas|                                     |
          |  +--------------------+                                     |
          +-------------------------------------------------------------+
```

### 5.2 Session Authentication Flow
```mermaid
sequence_diagram
autonumber
Client->>Server: [MeshCore Connect] Challenge Nonce (signed with client key)
Server->>Server: Verify signature with Client PubKey lookup
Server->>Client: [MeshCore Accept] Server Nonce (signed with server key) + DH PubKey
Client->>Client: Verify Server Signature
Client->>Server: Client DH PubKey (encrypted)
Server->>Server: Derives Ephemeral Symmetric Session Key
Client->>Client: Derives Ephemeral Symmetric Session Key
Server->>Client: Establish Encrypted Channel -> Enter configured main_app (e.g. dynamic main menu)
```

### 5.3 Passive Multicast Caching Flow
```mermaid
sequence_diagram
autonumber
Note over Client A: Logs in to BBS
Server->>Client A: [OP_RENDER_ASSET] [0x015A]
Note over Client A: Cache Miss for 0x015A
Client A->>Server: [REQ_ASSET] [0x015A]
Server->>RF Broadcast: Public Multicast Asset ID 0x015A (Chunk 1/3)
Client A->>Client A: Receives & caches Chunk 1
Client B (Idle): Intercepts & caches Chunk 1
Server->>RF Broadcast: Public Multicast Asset ID 0x015A (Chunk 2/3)
Client A->>Client A: Receives & caches Chunk 2
Client B (Idle): Intercepts & caches Chunk 2
Server->>RF Broadcast: Public Multicast Asset ID 0x015A (Chunk 3/3)
Client A->>Client A: Assembles & renders 0x015A
Client B (Idle): Assembles & commits 0x015A to SPI Flash
Note over Client B: User decides to log in to BBS later
Server->>Client B: [OP_RENDER_ASSET] [0x015A]
Client B->>Client B: Reads 0x015A from SPI Flash (Cache Hit!) -> Renders instantly
```

---

## 6. Open Questions & Future Scope

* **Multihop Asset Broadcasts:** How do we optimize the trickle broadcast across multiple repeater hops without flooding the entire mesh? Will need a hop-limiting mechanism or geographic bounding.
* **Differential Screen Updates on Dynamic Door Games:** Can we standardise a Lua coordinate diffing library to automate screen deltas, or should the developer manually write delta redraw logic?
* **C-based Client Implementation:** Need to draft a lightweight C-library for Heatshrink decompression and MeshANSI drawing to make client development on T-Deck / ESP32 simple.
* **Decentralized App Store & Catalog:** Official and community applications are hosted in standalone Git repositories (`bifrost-bbs/app-*`), indexed in `bifrost-bbs/app-catalog`, and managed through the Heimdall supervisor.

---

## 7. Acceptance Criteria for Release 1.0-alpha

1. **Rust Core Compilation:** Core compiles on target systems and runs without memory leaks.
2. **Mock Test Verification:** Can successfully run a simulated BBS session over the Local TCP harness with 10% simulated packet loss and verify that the session handles packet drops correctly.
3. **MeshANSI Validation:** An input ANSI screen translates to bytecode, compresses via Heatshrink, decompress on mock client, and accurately recreates the visual screen character-for-character.
4. **Rate Limiting Guard:** Sending continuous rapid commands triggers the queue throttling and respects the 350ms inter-packet guard window.
5. **Lua App Load & App Store:** Dynamic navigation menus, core message threads, user profile editing, and catalog-installed door games execute correctly within their memory quotas.
