# Agent Guidelines & Ways of Working (AGENTS.md)

Welcome, AI Coding Assistant! This document establishes the guidelines, constraints, and standard operating procedures for developing the **bifrost** project (implementing MeshBBS and MeshANSI). You must read and follow these rules at all times.

---

## 1. Project Context & Objectives

Bifrost is a decentralized Bulletin Board System (MeshBBS) designed for low-power, long-range (LoRa) mesh networks using the **MeshCore** protocol. It replaces standard dial-up BBS functions under physical limits:
*   **Extreme Bandwidth Constraints:** 250 bps – 5.4 kbps data rates.
*   **Duty-Cycle Regulatory Limits:** 1.0% limit (rolling window).
*   **Packet Fragmentation:** Enforced MTUs (~200 bytes).

To solve this, we use a custom binary bytecode compiler (**MeshANSI**), Heatshrink LZSS compression, and an **opportunistic client-side cache** for public assets.

---

## 2. Codebase Architecture

The project is structured as a Rust Cargo Workspace:

```
/bifrost/
├── Cargo.toml                       # Workspace Cargo configuration
├── .gitignore                       # Ignored build and test files
├── AGENTS.md                        # This developer rules file
├── README.md                        # Project high-level instructions
├── docs/
│   ├── MeshBBS_MeshANSI_Spec.md     # Protocol and architecture specification
│   └── PRD.md                       # Product Requirements Document
├── apps/                            # Encapsulated Lua BBS Applications
│   ├── main_menu/                   # Default navigation entry point (manifest.toml, main.lua, assets/)
│   ├── messages/                    # Discussion boards (manifest.toml, main.lua)
│   ├── marketplace/                 # Classifieds and auctions (manifest.toml, main.lua)
│   ├── minidungeon/                 # Asynchronous door game (manifest.toml, main.lua, assets/)
│   ├── profile/                     # Profile editor (manifest.toml, main.lua)
│   └── admin/                       # Admin console (manifest.toml, main.lua)
└── crates/
    ├── bifrost-compression/        # Heatshrink LZSS algorithm implementation
    │   ├── Cargo.toml
    │   └── src/
    ├── bifrost-ansi/               # Bytecode encoding, decoding, compression
    │   ├── Cargo.toml
    │   └── src/
    ├── bifrost-transport/          # Network framing, traits, and mock harness
    │   ├── Cargo.toml
    │   └── src/
    ├── bifrost-bbs/                # Main server host engine (tokio, mlua, sled)
    │   ├── Cargo.toml
    │   └── src/
    └── bifrost-client/             # Interactive client terminal emulator
        ├── Cargo.toml
        └── src/
```

---

## 3. Technology Stack & Coding Standards

### 3.1 Rust (Core Host Engine & Libraries)
*   **Rust Edition:** 2021.
*   **Asynchronous Runtime:** `tokio` (multi-threaded, high-concurrency).
*   **Safety:** Zero `unsafe` code allowed unless explicitly justified for low-level serial device access.
*   **Error Handling:** Use `thiserror` for library crates (`bifrost-ansi`, `bifrost-transport`, `bifrost-compression`) and `anyhow` for the application runner (`bifrost-bbs`, `bifrost-client`).
*   **Data Serialization:** Use `serde` for file/packet serialization where JSON or binary format is needed.

### 3.2 Lua (Application Framework)
*   **Lua Version:** 5.4.
*   **Sandbox Security:** Applications run within sandboxed `mlua` interpreters.
*   **Resource Bounds:** Execution slice limit of 500,000 instructions; memory heap limit of 512 KB.
*   **API Restriction:** Scripts must *only* interact via host APIs (`term`, `session`, `db`). No raw file I/O or system library access.

---

## 4. Specific Component Guidelines

### 4.1 MeshANSI Compiler
*   **Encoding Rules:** Compile ANSI escape sequences to the 1-byte opcode bytecode. Avoid sending raw escape bytes where opcodes exist.
*   **Delta Compression:** Prioritize delta updates. Compare the new screen buffer with the old client shadow buffer and only transmit the bounding boxes that have changed.
*   **Heatshrink Compression:** Apply LZSS compression using a sliding window size of $W=8$ (256 bytes) and lookahead size of $L=4$ (16 bytes).

### 4.2 Airtime Regulator & QoS
*   Calculate packet airtime ($T_{\text{air}}$) using the standard LoRa formulas based on Spreading Factor (SF), Bandwidth (BW), and preamble.
*   Respect the 5-Tier QoS queuing levels:
    1.  **Priority 0:** Connection negotiation & handshakes (ACKs).
    2.  **Priority 1:** Interactive keystrokes and cursor/delta feedback.
    3.  **Priority 2:** Full-screen state payloads.
    4.  **Priority 3:** Bulk data transfers (private mail, files).
    5.  **Priority 4:** On-demand public asset broadcasts.
*   Enforce a rolling average airtime usage cap (1.0% by default) using a leaky-bucket or token-bucket rate limiter.

### 4.3 Testing & CI
*   **Encapsulation & Testability:** Write highly modular, functional code to ensure easy testability and isolation.
*   **Coverage Target:** Maintain a minimum of **95% test coverage** across the cargo workspace.
*   **Unit Tests:** Place unit tests in a nested `tests` module in the same file as implementation:
    ```rust
    #[cfg(test)]
    mod tests {
        use super::*;
        // tests here
    }
    ```
*   **Integration Tests:** Place integration tests in the `tests/` directories utilizing the virtual mock radio socket harness.
*   **Verify Code & Coverage:**
    *   To run the test suite:
        ```bash
        cargo test
        ```
    *   To check source code coverage:
        ```bash
        cargo llvm-cov
        ```
    *   To generate an interactive HTML coverage report:
        ```bash
        cargo llvm-cov --html
        ```

