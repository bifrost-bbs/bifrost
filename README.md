# Bifrost: Decentralized BBS & MeshANSI over LoRa Mesh Networks

Bifrost is a next-generation Bulletin Board System (MeshBBS) and compression runtime (MeshANSI) engineered specifically for low-power, long-range (LoRa) mesh networks using the **MeshCore** protocol.

---

## 🚀 Key Features

*   **⚡ Multi-Tier Adaptive Compression Pipeline:**
    *   **MeshANSI Bytecode:** Replaces heavy ANSI escape codes with compact 1-byte opcodes.
    *   **Domain-Specific Dictionary Encoding:** Tokenizes high-frequency BBS phrases, UI headers, and opcode sequences into 1-byte tokens.
    *   **Heatshrink LZSS Compression:** Tuned sliding-window LZSS ($W=6..8, L=4..5$).
    *   **Compound Compression:** Combines domain dictionary tokenization with Heatshrink LZSS to achieve **+36.9% net airtime reduction**.
    *   **Anti-Expansion Guard:** Automatically falls back to raw transmission if compression yields negative gain.
*   **🔁 Session-Level Packet Deduplication (Hash-Referencing):**
    *   Caches recurring screen frames and menus in an LRU buffer.
    *   Repeated payloads are replaced with a 4-byte CRC32 reference packet (Sub-Header Flag `0x08`), cutting airtime by **90–98%** on repeated screens.
    *   Automatic NACK retransmission recovery if a client experiences a cache miss.
*   **💾 Passive Public Asset Multicast & Per-Node Caching:**
    *   Static assets (logos, menus, game art) and live domain dictionaries (Asset `0x00DF`) are broadcast as public unencrypted chunks.
    *   Nearby listening nodes passively cache them locally partitioned by BBS Node ID (`.client_cache/<node_id>/`), enabling zero-airtime rendering.
*   **⏱️ 10-Minute Seamless Session Resumption:**
    *   Preserves active session state and variables across transient disconnections within a 10-minute window, invoking `on_resume()` hooks.
*   **🏪 Decentralized App Catalog & Heimdall App Store:**
    *   Standalone apps live in independent GitHub repositories under the **`bifrost-bbs`** organization (`app-minidungeon`, `app-marketplace`, `app-weather`, `app-voidtrader`).
    *   Central verified catalog registry (`bifrost-bbs/app-catalog`) enables one-click installation, version updates, enabling/disabling, and removal via the Heimdall App Store.
*   **🌐 Multi-BBS Network & Relaying ("Bifrost Net"):**
    *   Federates local LoRa RF mesh communities across global internet backhauls (TCP/TLS).
    *   **Cryptographic Relay Framing (`BifrostRelayFrame`):** Authenticated end-to-end packet traversal preserving user Ed25519 node identities across intermediate transit hubs.
    *   **Central Network Registry:** Central verified node directory ([`bifrost-bbs/bbs-network-registry`](https://github.com/bifrost-bbs/bbs-network-registry)) with automated CI validation.
    *   **Multi-Hop Routing & Loop Detection:** Up to $N$ hops (`max_hops`) with cryptographic auth tags and active loop prevention.
    *   **Terminal & Web Hubs:** Built-in dynamic terminal navigation app (`[N] Network BBSs`) and Heimdall NOC peering dashboard with real-time latency ping testing.
*   **🛠️ Sandboxed Lua Application Engine:**
    *   Core apps (`messages`, `profile`, `admin`) and modular door games run within sandboxed `mlua` (Lua 5.4) instances with strict memory (512 KB) and instruction limits.
*   **🛡️ Regulatory QoS & Airtime Regulator:**
    *   Enforces rolling duty-cycle caps (1.0% by default) via token-bucket limiting and a 5-tier Priority Queue.
*   **🤖 Automated Crawler & Tuning Benchmark Suite (`bifrost-tuning`):**
    *   Client auto-navigator with inverse-visit weighted exploration to generate realistic test traffic.
    *   Dedicated tuning CLI (`bifrost-tuning`) supporting `analyze`, `train`, and `sweep` commands for optimizing parameters.

---

## 📂 Repository Layout

The project is managed as a Rust Cargo Workspace:

```
├── Cargo.toml                       # Cargo Workspace definition
├── README.md                        # Project documentation
├── AGENTS.md                        # AI coding assistant guidelines
├── docs/
│   ├── Lua_App_Development_Guide.md     # Lua application developer tutorial & API reference
│   ├── Multi_BBS_Network_Guide.md       # Multi-BBS network, relaying, and peering guide
│   ├── MeshBBS_MeshANSI_Requirements_Specification.md  # Protocol & architecture spec
│   └── PRD.md                       # Product Requirements Document
├── apps/                            # Core Built-in BBS Applications
│   ├── messages/                    # Discussion forums & boards
│   ├── profile/                     # User profile editor
│   └── admin/                       # Sysop admin console
├── assets/                          # Global shared system artwork & menus
├── config/                          # Default configurations & pre-trained dictionaries
│   └── bbs_dict.bin                 # Trained BBS domain dictionary (Asset 0x00DF)
└── crates/
    ├── bifrost-compression/        # Heatshrink LZSS & Domain Dictionary algorithms
    ├── bifrost-ansi/               # MeshANSI bytecode compiler & adaptive decompressor
    ├── bifrost-transport/          # Packet framing, session deduplication cache, stats
    ├── bifrost-bbs/                # Main host server daemon (kernel, Lua runner, packet capture)
    ├── bifrost-client/             # Interactive terminal emulator & automated crawler
    ├── bifrost-tuning/             # Parameter grid search & dictionary training CLI
    └── heimdall/                   # Master supervisor, NOC web dashboard & App Store
```
---

## 🛠️ Getting Started

### 📋 Prerequisites

To compile and run Bifrost:
*   **Rust:** Version 1.70+ (2021 Edition)
*   **Cargo:** Included with Rust
*   **C Compiler:** gcc/clang for C library bindings.

### 🔨 Building the Workspace

```bash
cargo build --release
```

### ⚙️ Initial Configuration Setup

Bifrost ships with an example configuration template. Copy it to create your local `config.toml` (which is gitignored so local configurations and installed catalog apps remain untouched):

```bash
# Copy example configuration template
cp config.example.toml config.toml
```

> [!NOTE]
> `config.toml` is gitignored to keep node operator credentials, form colors, and local settings private. The core applications (`apps/admin`, `apps/messages`, `apps/profile`) are committed to the repository, while external apps downloaded via the Heimdall App Store into `apps/<app_id>/` are automatically gitignored.

### 🧪 Running Tests & Code Coverage

```bash
# Run full unit and integration test suite
cargo test

# Generate coverage report
cargo llvm-cov --html
```

### 🏁 Launching Heimdall & Bifrost

```bash
# 1. Launch Heimdall Master Supervisor & Web NOC Dashboard (Port 9324)
cargo run --bin heimdall

# Or run Heimdall with custom options & authentication
cargo run --bin heimdall -- --port 9324 --user admin --pass secret

# 2. Open Web Admin Console in your browser
# -> http://localhost:9324

# 3. Direct BBS daemon & interactive terminal commands
cargo run --bin bifrost-bbs -- --config config.toml --mock --capture captured_packets
cargo run --bin bifrost-client
cargo run --bin bifrost-client -- --crawl --crawl-steps 100 --headless
```

### 🔬 Compression Tuning & Training

```bash
# Benchmark compression algorithms on captured traffic
cargo run --bin bifrost-tuning -- analyze --dir captured_packets/raw

# Run parameter sweep across Heatshrink window and lookahead settings
cargo run --bin bifrost-tuning -- sweep --dir captured_packets/raw

# Train a custom 254-token domain dictionary from captured packets
cargo run --bin bifrost-tuning -- train --dir captured_packets/raw --out config/bbs_dict.bin --tokens 254
```

---

## 📖 Documentation

*   **[Lua Application Development Guide](docs/Lua_App_Development_Guide.md):** Complete guide, tutorials, and host API reference (`term`, `session`, `db`, `log`, `http`) for developing MeshBBS apps.
*   **[Product Requirements Document (PRD)](docs/PRD.md):** Vision, functional requirements, and architecture diagrams.
*   **[Protocol & Architecture Specification](docs/MeshBBS_MeshANSI_Requirements_Specification.md):** Wire format, opcode tables, deduplication schemas, and compression pipelines.
*   **[Developer & Agent Guidelines](AGENTS.md):** Architecture rules and development SOPs.

