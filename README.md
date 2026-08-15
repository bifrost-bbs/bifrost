# Bifrost: Decentralized BBS & MeshANSI over LoRa Mesh Networks

Bifrost is a next-generation Bulletin Board System (MeshBBS) and compression runtime (MeshANSI) engineered specifically for low-power, long-range (LoRa) mesh networks using the **MeshCore** protocol.

---

## 🚀 Key Features

*   **⚡ Bandwidth-Optimized MeshANSI Protocol:** Replaces heavy, multi-byte ANSI escape codes with a high-density 1-byte opcode bytecode. Built-in **Heatshrink LZSS compression** ($W=8, L=4$) shrinks typical terminal screens down to 200–500 bytes.
*   **💾 Passive Public Asset Multicast:** Static visual elements (headers, border layouts, logos, game art) are requested by active clients and broadcast publicly by the server. Nearby listening client nodes passively intercept and cache these assets in local flash storage, enabling zero-airtime rendering in future sessions.
*   **🛠️ Sandboxed Lua Application Engine:** BBS apps (main menus, discussion forums, encrypted mail, door games) run dynamically in a memory-bounded (512 KB) and instruction-capped sandbox.
*   **🛡️ Regulatory QoS & Airtime Regulator:** Active tracking of LoRa transmitter duty cycle. Integrates a token-bucket rate limiter and a 5-tier Priority Queue (ACKs > Cursor Deltas > Screens > Bulk > On-Demand Broadcasts).
*   **🧪 Pluggable Transport Harness:** A simulation-ready architecture that supports production hardware (KISS/UART LoRa transceivers) as well as virtual TCP/Unix domain socket testing with synthetic packet loss and latency injection.

---

## 📂 Repository Layout

The project is managed as a Rust Cargo Workspace:

```
├── Cargo.toml                       # Cargo Workspace definition
├── README.md                        # This readme
├── AGENTS.md                        # AI coding assistant guidelines
├── docs/
│   ├── MeshBBS_MeshANSI_Requirements_Specification.md  # Core protocol spec
│   └── PRD.md                       # Product Requirements Document
├── apps/                            # Lua BBS applications
│   ├── 00_main_menu.lua             # Entry point
│   ├── 10_messages.lua              # Discussion boards
│   └── 30_doorgames/                # Asynchronous door games
│       └── minidungeon.lua
├── assets/                          # Shared visual assets
│   └── manifest.toml                # Asset registry
└── crates/
    ├── meshansi/                    # Bytecode encoding, decoding, and compression
    ├── meshcore-transport/          # Packet framing, serialization, and mock sockets
    └── meshbbs/                     # Main host server daemon (kernel, scheduler, Lua runner)
```

---

## 🛠️ Getting Started

### 📋 Prerequisites

To compile and run Bifrost, you need:
*   **Rust:** Version 1.70+ (2021 Edition)
*   **Cargo:** Included with Rust
*   **C Compiler:** (gcc/clang) for compiling dependencies like the Heatshrink bindings.

### 🔨 Building the Project

Compile the workspace binaries and libraries:
```bash
cargo build --release
```

### 🧪 Running the Test Suite & Coverage

Run unit and integration tests across the cargo workspace:
```bash
cargo test
```

To run tests and check code coverage (requires `cargo-llvm-cov` and `llvm-tools-preview`):
```bash
# 1. Install components (one-time setup)
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked

# 2. View coverage summary
cargo llvm-cov

# 3. Generate HTML coverage report
cargo llvm-cov --html
```

### 🏁 Launching the Mock Server

To launch the BBS server daemon using the simulated TCP socket transport for development:
```bash
cargo run --bin meshbbs -- --config config.toml --mock
```

---

## 📖 Documentation

For detailed specifications and design logs:
*   **[Product Requirements Document (PRD)](docs/PRD.md):** Vision, functional requirements, and architecture diagrams.
*   **[System Architecture & Protocol Specification](docs/MeshBBS_MeshANSI_Requirements_Specification.md):** Bytecode opcode tables, packet multiplexing formats, and rate limiter schemas.
*   **[Agent Guidelines (AGENTS.md)](AGENTS.md):** Detailed ways of working for coding assistants.

