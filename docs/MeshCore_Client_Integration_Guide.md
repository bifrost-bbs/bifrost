# MeshCore Client Implementation & Bifrost MeshBBS Complete Technical Specification

**Document Version:** 2.1.0  
**Target Audience:** Embedded firmware engineers, client app developers, and autonomous agents implementing a Bifrost MeshBBS client from scratch (e.g. on ESP32, nRF52840, LilyGO T-Deck, T-Echo, Linux/Android/iOS/Web).

---

## Table of Contents
1. [Physical Layer & Default Radio Parameters](#1-physical-layer--default-radio-parameters)
2. [Serial Interface & KISS Modem Protocol](#2-serial-interface--kiss-modem-protocol)
3. [MeshBBS Network Framing & Reassembly](#3-meshbbs-network-framing--reassembly)
4. [Compression & Decompression Engines](#4-compression--decompression-engines)
5. [Terminal Display & Character Set Architecture](#5-terminal-display--character-set-architecture)
6. [Color Palette & Attribute Encoding](#6-color-palette--attribute-encoding)
7. [MeshANSI Bytecode Opcode Specification](#7-meshansi-bytecode-opcode-specification)
8. [Interactive Form & Widget State Machine](#8-interactive-form--widget-state-machine)
9. [Opportunistic Asset Caching & Template System](#9-opportunistic-asset-caching--template-system)
10. [End-to-End Client Lifecycle Walkthrough](#10-end-to-end-client-lifecycle-walkthrough)
11. [Repository Sample Code Reference Links](#11-repository-sample-code-reference-links)
12. [Reference Vectors & Verification Checksums](#12-reference-vectors--verification-checksums)

---

## 1. Physical Layer & Default Radio Parameters

Bifrost operates over low-power LoRa mesh transceivers (e.g., Semtech SX1262 / SX1276).

### 1.1 New Zealand Standard MeshCore Preset (Default)
When initializing or configuring the radio hardware for standard operation, use the following default parameters:

| Parameter | Value | Notes |
|---|---|---|
| **Frequency (`frequency_hz`)** | `917375000` (917.375 MHz) | NZ ISM Band Channel |
| **Bandwidth (`bandwidth_hz`)** | `62500` (62.5 kHz) | Narrowband for extended link margin |
| **Spreading Factor (`sf`)** | `7` | Balanced rate vs. sensitivity |
| **Coding Rate (`cr`)** | `5` (4/5) | Standard MeshCore forward error correction |
| **TX Power (`tx_power_dbm`)** | `22` dBm | Maximum legal EIRP for handhelds/nodes |
| **Preamble Length** | `8` symbols | Standard LoRa preamble |
| **Airtime Factor (`af`)** | `9` | MeshCore duty-cycle regulatory airtime factor |

---

## 2. Serial Interface & KISS Modem Protocol

If the client connects to an external MeshCore modem (T-Beam, RAK4631, Heltec) via UART or USB CDC serial:

### 2.1 Serial Line Configuration
* **Baud Rate:** `115200`
* **Data Bits:** `8`, **Stop Bits:** `1`, **Parity:** `None` (8N1)
* **Flow Control:** None

### 2.2 KISS Framing (KA9Q / K3MC)
Packets sent to and from the serial modem are delimited by `FEND` (`0xC0`).

```
┌──────┬───────────┬──────────────────────────────────────────┬──────┐
│ FEND │ Type Byte │ Data Payload (with byte escaping)        │ FEND │
│ 0xC0 │   0x00    │ Escaped stream (0 - 255 bytes)           │ 0xC0 │
└──────┴───────────┴──────────────────────────────────────────┴──────┘
```

#### Byte Escaping Rules:
- If byte is `0xC0` (`FEND`), transmit two bytes: `0xDB 0xDC` (`FESC` + `TFEND`).
- If byte is `0xDB` (`FESC`), transmit two bytes: `0xDB 0xDD` (`FESC` + `TFESC`).

#### KISS Type Bytes:
- `0x00` (`KISS_CMD_DATA`): Outbound payload to transmit over the air, or incoming packet received over RF.
- `0x06` (`KISS_CMD_SETHARDWARE`): Hardware query/command interface.

#### Essential Modem Hardware Commands (`Type = 0x06`):
- `0x01` (`GetIdentity`): Queries local 32-byte Ed25519 public key. Response: `0x81 <32-byte PubKey>`.
- `0x09` (`SetRadio`): `0x09 <Freq: 4B LE> <BW: 4B LE> <SF: 1B> <CR: 1B>`. Response: `0xF0` (OK).
- `0x0A` (`SetTxPower`): `0x0A <Power_dBm: 1B>`. Response: `0xF0` (OK).
- `0x19` (`SetSignalReport`): `0x19 0x01` (Enables SNR/RSSI metadata callbacks `0xF9 <SNR: 1B> <RSSI: 1B>`).

---

## 3. MeshBBS Network Framing & Reassembly

All Bifrost packets carry the dedicated application port header `0xBB` (`MESHBBS_APP_PORT`).

### 3.1 Network Fragment Header (4 Bytes)
Because LoRa packets are fragmented to comply with link MTU (typically $\le 200 - 250$ bytes), every over-the-air packet starts with:

```
Byte 0: 0xBB (Application Port = MESHBBS_APP_PORT)
Byte 1: Channel / Session Flag (0x00 for broadcast, or 1-byte session stream index)
Byte 2: Frame Sequence (1-indexed chunk number: 1, 2, ..., N)
Byte 3: Total Chunks (Total fragment count N: 1 <= N <= 255)
Byte 4..: Fragment Payload Slice
```

### 3.2 Inner Message Header (Reassembled Buffer)
Once all `N` fragments for a message are received, concatenate the fragment payload slices in order. The inner message buffer starts with a 5-byte header:

```
Byte 0:    Opcode (1 Byte)
Byte 1:    Flags (1 Byte)
Byte 2:    Payload Length (1 Byte, 0..255)
Byte 3..4: CRC16-CCITT (2 Bytes, Big-Endian, Polynomial 0x1021, Seed 0xFFFF)
Byte 5..:  Payload Data (N Bytes)
```

#### Message Opcodes:
- `0x01` (`OP_HANDSHAKE`): Connect / Session resume request.
- `0x02` (`OP_DISCONNECT`): Graceful session disconnect.
- `0x03` (`OP_DATA`): Terminal screen bytecode / delta stream.
- `0x04` (`OP_INPUT`): Keystroke or Form JSON submission.
- `0x05` (`OP_REQ_ASSET`): Missing asset retransmission request (`AssetID: 2B`).
- `0x06` (`OP_NACK_CRC`): Dedup cache miss NACK (`CRC32: 4B`).

#### Sub-Header Flags (`Byte 1`):
- `0x00`: Uncompressed Raw Bytecode.
- `0x02`: Heatshrink LZSS Compressed ($W=8, L=4$).
- `0x04`: Pre-trained Domain Dictionary Compressed.
- `0x06`: Combined Dictionary + Heatshrink Compressed.
- `0x08`: Session Deduplication Reference (Payload contains 4-byte CRC32 hash of identical cached frame).
- `0x10`: Public Multicast Asset Chunk (For opportunistic client caching).

---

## 4. Compression & Decompression Engines

Bifrost uses a multi-tier adaptive compression pipeline to minimize airtime:

### 4.1 Heatshrink LZSS Decompression
- **Window Size ($W$):** `8` (256-byte sliding window).
- **Lookahead Size ($L$):** `4` (16-byte lookahead buffer).
- When `(flags & 0x02) != 0`, feed the payload through standard Heatshrink stream decoder.

### 4.2 Pre-Trained Domain Dictionary Decompression
When `(flags & 0x04) != 0`, the bytecode uses token substitutions:
- Escape Byte: `0xFD`
- `0xFD <token_idx>`: Replace with token string at `tokens[token_idx]`.
- `0xFD 0xFF`: Literal `0xFD` byte in stream.

### 4.3 Session Frame Deduplication
- When `(flags & 0x08) != 0`, the server is referencing a previously sent full screen frame to save 100% of airtime.
- The 4-byte payload is `CRC32_BE`.
- If found in local client session LRU cache: render the cached buffer immediately.
- If not found (cache miss): send `Opcode 0x06` (`OP_NACK_CRC`) containing the missing `CRC32_BE` to request immediate full retransmission.

---

## 5. Terminal Display & Character Set Architecture

### 5.1 Screen Canvas Dimensions
A Bifrost client terminal emulator must support two responsive layout modes:
1. **Full Standard Layout:** 80 columns $\times$ 25 rows.
2. **Compact Handheld Layout:** 40 columns $\times$ 25 rows (optimized for LilyGO T-Deck / mobile).

### 5.2 Character Set: IBM Code Page 437 (CP437)
The terminal canvas renders characters from the standard IBM PC CP437 character set. For modern Unicode/UTF-8 graphical engines (SDL2, LVGL, Canvas, WebGL), use the following mapping table:

| Hex Byte | CP437 Glyph | Unicode Point | Description |
|---|---|---|---|
| `0x20..0x7E` | `ASCII` | `U+0020..U+007E` | Standard printable ASCII characters |
| `0xB0` | `░` | `U+2591` | Light Shade |
| `0xB1` | `▒` | `U+2592` | Medium Shade |
| `0xB2` | `▓` | `U+2593` | Dark Shade |
| `0xB3` | `│` | `U+2502` | Box Drawings Light Vertical |
| `0xB4` | `┤` | `U+2524` | Box Drawings Light Vertical and Left |
| `0xBA` | `║` | `U+2551` | Box Drawings Double Vertical |
| `0xBB` | `╗` | `U+2557` | Box Drawings Double Down and Left |
| `0xBC` | `╝` | `U+255D` | Box Drawings Double Up and Left |
| `0xC4` | `─` | `U+2500` | Box Drawings Light Horizontal |
| `0xC5` | `┼` | `U+253C` | Box Drawings Light Vertical and Horizontal |
| `0xCD` | `═` | `U+2550` | Box Drawings Double Horizontal |
| `0xC8` | `╚` | `U+255A` | Box Drawings Double Up and Right |
| `0xC9` | `╔` | `U+2554` | Box Drawings Double Down and Right |
| `0xDB` | `█` | `U+2588` | Full Block |
| `0xDC` | `▄` | `U+2584` | Lower Half Block |
| `0xDF` | `▀` | `U+2580` | Upper Half Block |
| `0xFE` | `■` | `U+25A0` | Black Small Square |

---

## 6. Color Palette & Attribute Encoding

Bifrost uses the classic 16-color CGA / ANSI palette. Colors are packed into a single byte attribute:
- **High Nibble (`attr >> 4`):** Background Color (0..15)
- **Low Nibble (`attr & 0x0F`):** Foreground Color (0..15)

```
Byte: [ BG_3 | BG_2 | BG_1 | BG_0 | FG_3 | FG_2 | FG_1 | FG_0 ]
```

### 6.1 16-Color RGB Palette Table

| Index | Name | Hex RGB | Preview / Usage |
|---|---|---|---|
| `0` | Black | `#000000` | Default Background |
| `1` | Blue | `#0000AA` | Primary Dark Accent |
| `2` | Green | `#00AA00` | Status OK |
| `3` | Cyan | `#00AAAA` | Secondary Accent |
| `4` | Red | `#AA0000` | Errors / Form Field BG |
| `5` | Magenta | `#AA00AA` | Special Highlighting |
| `6` | Brown / Dark Yellow | `#AA5500` | Warning Dim |
| `7` | Light Gray | `#AAAAAA` | Default Foreground / Button BG |
| `8` | Dark Gray | `#555555` | Borders / Inactive Text |
| `9` | Bright Blue | `#5555FF` | Active Links |
| `10` | Bright Green | `#55FF55` | Verified / Success |
| `11` | Bright Cyan | `#55FFFF` | Information Headers |
| `12` | Bright Red | `#FF5555` | Critical Alerts |
| `13` | Bright Magenta | `#FF55FF` | Special Elements |
| `14` | Yellow | `#FFFF55` | Titles / Highlights |
| `15` | Bright White | `#FFFFFF` | Form Text / High Contrast |

---

## 7. MeshANSI Bytecode Opcode Specification

The client terminal interprets a stream of decompressed bytes. The opcodes are defined as:

### 7.1 Control & Flow Opcodes
* `0x00` (`OP_NOP`): No operation.
* `0x01` (`OP_CLEAR_SCREEN`): Clears the terminal canvas, resets cursor to `(0, 0)`, resets active form state.
* `0x02` (`OP_CRLF`): Moves cursor to column `0` on the next row (`row += 1`).
* `0x03` (`OP_PAGE_PAUSE`): Displays `[Press Any Key]` prompt and suspends rendering until a keypress is received.
* `0x04` (`OP_END_OF_FRAME`): Frame flush commit. Tells display driver to repaint framebuffer to physical screen.

### 7.2 Cursor & Color Opcodes
* `0xC0` (`OP_SET_COLOR`): `0xC0 <Attr: 1B>`. Updates active foreground and background colors.
* `0xC1` (`OP_RLE_GLYPH`): `0xC1 <Count: 1B> <CP437_Glyph: 1B>`. Prints glyph `Count` times.
* `0xC2` (`OP_RLE_SPACE`): `0xC2 <Count: 1B>`. Advances cursor `Count` spaces (filled with current BG color).
* `0xC3` (`OP_CURSOR_ABS`): `0xC3 <Col: 1B> <Row: 1B>`. Positions cursor at absolute coordinate `(Col, Row)`.
* `0xC4` (`OP_CURSOR_REL`): `0xC4 <dCol: i8> <dRow: i8>`. Relative cursor movement.
* `0xC6` (`OP_DELTA_BLOCK`): `0xC6 <Col: 1B> <Row: 1B> <Width: 1B> <Height: 1B>`. Defines bounding box for subsequent differential character stream.

### 7.3 High-Level Asset & Template Opcodes
* `0xC5` (`OP_RENDER_ASSET`): `0xC5 <AssetID: 2B BE>`. Renders pre-cached bytecode file `assets/<AssetID>.ans` at current position. If missing from cache, send `Opcode 0x05` to request on-demand broadcast.
* `0xC7` (`OP_RENDER_TEMPLATE`): `0xC7 <AssetID: 2B BE> <ParamCount: 1B> [<Len: 1B> <StringData>]*`. Renders cached template string, replacing `{0}`, `{1}`... with parameters.
* `0xC8` (`OP_RENDER_MENU`): `0xC8 <AssetID: 2B BE> <ToggleMask: 4B BE>`. Parses and activates interactive menu form defined in cached CSV asset.

### 7.4 Form & Input Opcodes
* `0xD0` (`OP_FORM_START`): `0xD0 <FormID: 1B> <FieldFG: 1B> <FieldBG: 1B> <SubmitFG: 1B> <SubmitBG: 1B>`. Starts interactive form mode.
* `0xD1` (`OP_FORM_FIELD`): `0xD1 <Col: 1B> <Row: 1B> <Width: 1B> <ID_Len: 1B> <ID_Str> <Val_Len: 1B> <Val_Str>`. Defines an editable text field.
* `0xD2` (`OP_FORM_SUBMIT`): `0xD2 <Col: 1B> <Row: 1B> <ID_Len: 1B> <ID_Str>`. Defines a focusable submit button widget.
* `0xD3` (`OP_FORM_END`): Finalizes form definition. Sets focus to first field or submit button.

---

## 8. Interactive Form & Widget State Machine

When `OP_FORM_START` is encountered, the client maintains a local form state:

```rust
struct FormField {
    id: String,
    col: u8,
    row: u8,
    width: u8,
    height: u8,
    val: String,
    is_submit: bool,
    key: Option<char>, // Hotkey accelerator (e.g. '1', 'Q', 'M')
}
```

### 8.1 Key Navigation Rules
- **Tab / Down Arrow / Right Arrow:** Move focus to next form field / button (`active_idx = (active_idx + 1) % fields.len()`).
- **Shift+Tab / Up Arrow / Left Arrow:** Move focus to previous form field / button.
- **Printable Characters on Text Field:** Appends character to `field.val` up to `field.width` and redraws field locally.
- **Backspace on Text Field:** Deletes last character in `field.val` and updates field locally.
- **Enter on Submit Button / Hotkey Press:**
  1. Assemble JSON submission object containing all input values and the triggered submit button ID:
     ```json
     {
       "nickname": "Alice",
       "message": "Hello mesh!",
       "submit": "btn_post"
     }
     ```
  2. Transmit message with `Opcode 0x04` (`OP_INPUT`) containing the UTF-8 JSON payload to the BBS.

---

## 9. Opportunistic Asset Caching & Template System

To achieve near-zero airtime for static screens, menus, and ANSI art banners:

1. **Promiscuous Asset Reception:**
   When listening on the mesh, any packet received with flag `0x10` (`FLAG_BROADCAST_ASSET`) contains public broadcast asset chunks. Reassemble the chunks, compute `CRC32`, and save locally to flash/storage (`/assets/<AssetID>.ans`).

2. **On-Demand Asset Fetching:**
   When encountering opcode `0xC5` or `0xC7` for an asset ID not found in local cache:
   - Client emits `Opcode 0x05` (`OP_REQ_ASSET`) with payload `<AssetID: 2B BE>`.
   - BBS rate-limiter schedules a public multicast broadcast of the requested asset.
   - Client receives the asset, caches it, and paints the screen.

---

## 10. End-to-End Client Lifecycle Walkthrough

```
 CLIENT DEVICE                                              BIFROST BBS HOST
 (e.g. T-Deck)                                              (LilyGo T-Beam)
      |                                                            |
      | 1. Query local modem identity (0x06 0x01)                  |
      |----------------------------------------->                  |
      | 2. Handshake: Opcode 0x01 [ClientPubKey + DeviceInfo]      |
      |===========================================================>|
      |                                                            | 3. Authenticate / Resume session
      |                                                            | 4. Compile Lua screen to MeshANSI
      | 5. MeshBBS Data: Opcode 0x03 [MeshANSI Bytecode Stream]    | 5. Heatshrink compress (W=8, L=4)
      |<===========================================================|
      |                                                            |
      | 6. Decompress Heatshrink bytecode                          |
      | 7. Render CP437 glyphs & forms to 80x25 canvas             |
      |                                                            |
      | [User navigates menu & presses Enter on "Read Messages"]   |
      |                                                            |
      | 8. Input Event: Opcode 0x04 {"submit": "btn_messages"}     |
      |===========================================================>|
      |                                                            | 9. Process event in Lua sandbox
      | 10. MeshBBS Data: Opcode 0x03 [Delta updates / New view]   | 10. Send differential response
      |<===========================================================|
```

---

## 11. Repository Sample Code Reference Links

Developers can inspect working reference implementations directly in the Bifrost codebase:

### 11.1 CLI Interactive & Headless Client (Rust + Crossterm)
- **Source File:** [`crates/bifrost-client/src/main.rs`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs)
- **Key Modules & Functions:**
  - `interpret_bytecode`: Bytecode parsing, cursor control, asset dispatch, and ANSI rendering ([`bifrost-client/src/main.rs:L1289-L1490`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs#L1289-L1490)).
  - `FormState` & `render_field_local`: Interactive form widget management and local keystroke feedback ([`bifrost-client/src/main.rs:L46-L71`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs#L46-L71), [`L1250-L1278`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs#L1250-L1278)).
  - `get_viewport_offsets`: Responsive 80x25 Full vs. 40x25 Compact layout viewport centering ([`bifrost-client/src/main.rs:L1120-L1140`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs#L1120-L1140)).
  - `interpret_bytecode_headless`: Autonomous stateful crawler / headless testing harness ([`bifrost-client/src/main.rs:L1492-L1620`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-client/src/main.rs#L1492-L1620)).

### 11.2 Web Virtual Terminal Client (Rust Backend + HTML5 Canvas Frontend)
- **Backend Bridge:** [`crates/heimdall/src/web_client.rs`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/src/web_client.rs)
  - `CP437_MAP`: Complete 256-entry array converting CP437 character indices to Unicode glyphs ([`heimdall/src/web_client.rs:L16-L33`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/src/web_client.rs#L16-L33)).
  - `VirtualTerminalCanvas`: 80x25 / 40x25 cell matrix holding character and color attributes, handling ANSI sequences and form field overlays ([`heimdall/src/web_client.rs:L85-L350`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/src/web_client.rs#L85-L350)).
  - `handle_client_terminal_ws`: Full WebSocket bridge running a virtual BBS client session over radio transport ([`heimdall/src/web_client.rs:L420-L750`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/src/web_client.rs#L420-L750)).
- **Frontend Canvas Renderer:** [`crates/heimdall/web/app.js`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/web/app.js)
  - `renderTerminalScreen`: HTML5 Canvas rendering engine drawing CP437 text grid and cursor positioning ([`heimdall/web/app.js:L1750-L1880`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/web/app.js#L1750-L1880)).
  - `sendTerminalKey`: Keyboard capture, hotkey accelerators, and tab navigation event forwarder ([`heimdall/web/app.js:L1885-L1940`](file:///home/pmumby/Development/Personal/bifrost/crates/heimdall/web/app.js#L1885-L1940)).

### 11.3 Transport & Modem Framing Layer
- **Source File:** [`crates/bifrost-transport/src/lib.rs`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-transport/src/lib.rs)
  - `encode_kiss_frame` & `KissFrameDecoder`: Serial KISS framing and stream unescaping state machine ([`bifrost-transport/src/lib.rs:L625-L703`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-transport/src/lib.rs#L625-L703)).
  - `KissModemTransport`: Async serial modem transceiver with telemetry polling and duty-cycle tracking ([`bifrost-transport/src/lib.rs:L734-L960`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-transport/src/lib.rs#L734-L960)).
  - `MeshBbsMessage::to_fragments` & `MessageReassembler`: MTU fragmentation, reassembly, CRC16 verification ([`bifrost-transport/src/lib.rs:L1110-L1280`](file:///home/pmumby/Development/Personal/bifrost/crates/bifrost-transport/src/lib.rs#L1110-L1280)).

---

## 12. Reference Vectors & Verification Checksums

When implementing codecs, verify with these test vectors:

- **CRC16-CCITT:**
  - Input: `b"123456789"`
  - Expected: `0x29B1` (Polynomial `0x1021`, Initial `0xFFFF`, no final XOR).
- **CRC32 (IEEE 802.3):**
  - Input: `b"123456789"`
  - Expected: `0xCBF43926` (Polynomial `0xEDB88320`, Initial `0xFFFFFFFF`, Inverted output).
- **KISS Framing Escape:**
  - Raw Input: `[0x00, 0xC0, 0xDB, 0x12]`
  - Framed Output: `[0xC0, 0x00, 0xDB, 0xDC, 0xDB, 0xDD, 0x12, 0xC0]`
