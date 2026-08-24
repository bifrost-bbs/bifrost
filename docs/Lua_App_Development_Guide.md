# Bifrost Lua Application Development Guide

Welcome to the **Bifrost MeshBBS Lua Application Framework** developer guide. This document provides a complete reference and tutorial for building decentralized, low-bandwidth applications (door games, message boards, classifieds, utilities, and BBS tools) running on Bifrost MeshBBS over LoRa mesh networks.

---

## 📑 Table of Contents

1. [Architecture & Physical Constraints](#1-architecture--physical-constraints)
2. [Application Directory Anatomy](#2-application-directory-anatomy)
3. [Quick Start: Hello World](#3-quick-start-hello-world)
4. [Tutorial: Building a Weather Info Fetch App](#4-tutorial-building-a-weather-info-fetch-app)
5. [Complete Host API Reference](#5-complete-host-api-reference)
   * [Display & Terminal (`term`)](#51-display--terminal-term)
   * [Session & Lifecycle (`session`)](#52-session--lifecycle-session)
   * [Persistent Storage (`db`)](#53-persistent-storage-db)
   * [System Logging (`log`)](#54-system-logging-log)
   * [HTTP & External Data (`http`)](#55-http--external-data-http)
6. [Declarative UI: Forms & Menus](#6-declarative-ui-forms--menus)
7. [Asset Management & Caching](#7-asset-management--caching)
8. [Session Lifecycle & Resumption Hooks](#8-session-lifecycle--resumption-hooks)
9. [Airtime & Performance Best Practices](#9-airtime--performance-best-practices)

---

## 1. Architecture & Physical Constraints

Bifrost operates under extreme physical network constraints:
* **Physical Bandwidth:** 250 bps – 5.4 kbps (LoRa Spreading Factors SF7–SF12).
* **Packet Size (MTU):** ~200 payload bytes per packet.
* **Duty Cycle Limits:** 1.0% regulatory transmission limit.

To maintain interactive terminal performance under these conditions, Bifrost executes Lua apps on the host server inside a hardened **`mlua` (Lua 5.4)** sandbox, compiling screen updates into a compact 1-byte binary bytecode format (**MeshANSI**).

```
┌────────────────────────────────────────────────────────┐
│               Bifrost Host (Rust Engine)               │
│                                                        │
│   ┌────────────────────────────────────────────────┐   │
│   │           Sandboxed Lua 5.4 App VM             │   │
│   │                                                │   │
│   │   term.*      session.*    db.*     log.*      │   │
│   └───────┬───────────┬──────────┬────────┬────────┘   │
│           │           │          │        │            │
│   ┌───────▼───────────▼──────────▼────────▼────────┐   │
│   │          MeshANSI Bytecode Compiler            │   │
│   └───────────────────┬────────────────────────────┘   │
│                       │ (Bytecode Buffer)              │
│   ┌───────────────────▼────────────────────────────┐   │
│   │    Adaptive Compression (Dictionary + LZSS)    │   │
│   └───────────────────┬────────────────────────────┘   │
└───────────────────────┼────────────────────────────────┘
                        │ LoRa Mesh Packets (MTU ~200B)
┌───────────────────────▼────────────────────────────────┐
│               MeshCore Client Terminal                 │
│   • Client-Side Asset Cache (.client_cache/<node_id>/) │
│   • 80x25 CP437 ANSI Virtual Screen Buffer             │
└────────────────────────────────────────────────────────┘
```

### 🔒 Sandbox Security Rules
* Standard dangerous libraries (`os`, `io`, `package`, raw file access) are completely removed.
* File loading is restricted to relative paths within the active app folder via `require("module")` or `session.include("file.lua")`.
* Scripts are bounded by strict memory heap limits (512 KB) and instruction slice limits (500,000 instructions) to prevent infinite loops.

---

## 2. Application Directory Anatomy

Every BBS application lives in an isolated folder under `apps/<app_id>/`:

```
/bifrost/apps/my_app/
├── manifest.toml        # Application metadata & asset declarations
├── main.lua             # Primary entry point
├── helpers.lua          # Optional scoped Lua submodules
└── assets/              # Static artwork, menus, and templates
    ├── banner.ans       # 80x25 CP437 ANSI artwork
    ├── nav_menu.csv     # Declarative menu definition
    └── prompt.ans       # Templated text screen
```

### 📄 Manifest Configuration (`manifest.toml`)

The manifest defines your app's ID, version, entry file, and static assets:

```toml
[app]
id = "my_app"
name = "My Space Station"
description = "Interactive station management door game"
author = "Your CallSign / Name"
version = "1.0.0"
entry_point = "main.lua"

[[assets]]
name = "station_banner"
path = "assets/banner.ans"

[[assets]]
name = "station_menu"
path = "assets/nav_menu.csv"
```

> **Note on Asset IDs:** You never need to hardcode global 16-bit Asset IDs. The Bifrost host automatically allocates collision-free 16-bit IDs on startup and transmits them to connecting clients.

---

## 3. Quick Start: Hello World

Let's build a minimal interactive application from scratch.

### Step 1: Create Directory and Manifest

Create `apps/hello_world/manifest.toml`:

```toml
[app]
id = "hello_world"
name = "Hello World"
description = "A minimal starter application for Bifrost BBS"
author = "Mesh Operator"
version = "1.0.0"
entry_point = "main.lua"
```

### Step 2: Write `apps/hello_world/main.lua`

An application module returns a table containing lifecycle hook functions:

```lua
local app = {}

function app.on_start(session)
    local user_id = session.node_id()
    local callsign = session.callsign()

    -- 1. Clear terminal screen
    term.clear()

    -- 2. Move cursor to Column 2, Row 2
    term.move_to(2, 2)

    -- 3. Set Color: Foreground 14 (Yellow), Background 1 (Blue)
    term.set_color(14, 1)
    term.print("╔═══════════════════════════════════════╗\n")
    term.move_to(2, 3)
    term.print("║       WELCOME TO BIFROST MESHBBS      ║\n")
    term.move_to(2, 4)
    term.print("╚═══════════════════════════════════════╝\n")

    -- 4. Reset colors to Light Gray on Black
    term.set_color(7, 0)
    term.move_to(2, 6)
    term.print("Hello, " .. callsign .. "!\n")
    term.move_to(2, 7)
    term.print("Your Node ID: " .. string.sub(user_id, 1, 16) .. "...\n\n")

    -- 5. Define an interactive single-button form
    term.define_form(1)
    term.add_submit_button("continue", 2, 10)
    term.flush_form()

    -- 6. Await user button press
    session.await_input(1, function(submission)
        -- Return to main menu on submission
        session.load_app("main_menu")
    end)
end

return app
```

### Step 3: Enable the App in `config.toml`

Add `"hello_world"` to the `[apps]` section in `config.toml`:

```toml
[apps]
main_app = "main_menu"
enabled = [
    "main_menu",
    "messages",
    "marketplace",
    "minidungeon",
    "profile",
    "admin",
    "weather",
    "hello_world",
]
```

---

## 4. Tutorial: Building a Weather Info Fetch App

This example shows how to query external network services, read client GPS coordinates, and render structured CP437 table data.

### `apps/weather_station/manifest.toml`

```toml
[app]
id = "weather_station"
name = "Weather Station"
description = "Fetches live weather for the radio node's coordinates"
author = "Bifrost Network"
version = "1.0.0"
entry_point = "main.lua"
```

### `apps/weather_station/main.lua`

```lua
local weather_app = {}

function weather_app.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0) -- Cyan
    term.print("=== SATELLITE WEATHER ORACLE ===\n\n")
    term.set_color(7, 0)

    -- Verify node GPS coordinates from MeshCore adverts
    if not user or not user.latitude or not user.longitude then
        term.print("No GPS coordinates found in your node advert packet.\n")
        term.print("Enable location broadcasting on your radio to use this feature.\n\n")

        term.define_form(1)
        term.add_submit_button("back", 2, 8)
        term.flush_form()

        session.await_input(1, function()
            session.load_app("main_menu")
        end)
        return
    end

    local lat = user.latitude
    local lon = user.longitude
    term.print(string.format("Position: %.4f Lat, %.4f Lon\n\n", lat, lon))

    -- Fetch forecast JSON from whitelisted endpoint
    local url = string.format(
        "https://api.open-meteo.com/v1/forecast?latitude=%f&longitude=%f&current_weather=true",
        lat, lon
    )
    local data = http.get_json(url)

    if not data or not data.current_weather then
        term.print("Satellite uplink timeout. Could not retrieve weather data.\n\n")
    else
        local cw = data.current_weather

        -- Render high-efficiency formatted table
        term.render_table(2, 6, {
            headers = { "Measurement", "Current Value" },
            widths = { 18, 22 },
            rows = {
                { "Temperature", string.format("%.1f °C", cw.temperature) },
                { "Wind Speed", string.format("%.1f km/h", cw.windspeed) },
                { "Wind Bearing", string.format("%d°", cw.winddirection) },
                { "Weather Code", tostring(cw.weathercode) }
            },
            header_fg = 14, -- Yellow
            header_bg = 0,
            row_fg = 15,    -- Bright White
            row_bg = 0,
            divider = true
        })
    end

    -- Return navigation form
    term.define_form(2)
    term.add_submit_button("back", 2, 14)
    term.flush_form()

    session.await_input(2, function(submission)
        session.load_app("main_menu")
    end)
end

return weather_app
```

---

## 5. Complete Host API Reference

### 5.1 Display & Terminal (`term`)

The `term` global table controls screen output, color palettes, form definitions, and cached asset rendering.

| Function | Arguments | Description |
| :--- | :--- | :--- |
| `term.clear()` | `()` | Clears the 80x25 screen and resets the cursor to `(1, 1)`. |
| `term.move_to(col, row)` | `(col: integer, row: integer)` | Moves the cursor to 1-indexed column (`1..80`) and row (`1..25`). |
| `term.set_cursor(col, row)` | `(col: integer, row: integer)` | Alias for `term.move_to`. |
| `term.set_color(fg, bg)` | `(fg: integer, bg: integer)` | Sets active text color using 16-color ANSI codes (`0..15`). |
| `term.print(text)` | `(text: string)` | Appends raw text or CP437 characters to the active output buffer. |
| `term.render_asset(name)` | `(name: string)` | Emits an asset reference opcode (`0xC5`) to render a cached ANSI screen with zero airtime. |
| `term.render_template(name, params)` | `(name: string, params: table\|string\|number)` | Renders a cached template asset replacing `%s` / `{1}` tokens with dynamic strings. |
| `term.render_menu(name, [toggle_mask])` | `(name: string, [toggle: integer\|table])` | Renders a declarative CSV navigation menu with optional button enable/disable masks. |
| `term.render_table(col, row, config)` | `(col: integer, row: integer, config: table)` | Renders a formatted data table with column widths and headers. |
| `term.flush()` | `()` | Compresses and transmits all buffered output to the client. |
| `term.define_form(form_id, [f_fg, f_bg, s_fg, s_bg])` | `(form_id: integer, ...)` | Begins a declarative input form with optional color styling. |
| `term.add_input_field(id, col, row, width, default)` | `(id: string, col: integer, row: integer, width: integer, default: string)` | Adds a single-line editable text input field. |
| `term.add_multiline_field(id, col, row, width, height, default)` | `(id: string, col: integer, row: integer, width: integer, height: integer, default: string)` | Adds a multiline textarea input field. |
| `term.add_submit_button(id, col, row)` | `(id: string, col: integer, row: integer)` | Adds a form submission button. |
| `term.flush_form()` | `()` | Finalizes the active form definition, compresses it, and transmits it. |

#### Color Code Palette Reference (`0..15`)
* `0`: Black | `1`: Blue | `2`: Green | `3`: Cyan | `4`: Red | `5`: Magenta | `6`: Brown/Orange | `7`: Light Gray
* `8`: Dark Gray | `9`: Bright Blue | `10`: Bright Green | `11`: Bright Cyan | `12`: Bright Red | `13`: Bright Magenta | `14`: Yellow | `15`: Bright White

---

### 5.2 Session & Lifecycle (`session`)

The `session` global table manages client identity, user inputs, permissions, script inclusions, and application navigation.

| Function | Arguments | Returns | Description |
| :--- | :--- | :--- | :--- |
| `session.node_id()` | `()` | `string` | Returns the 64-character hexadecimal representation of the client's 32-byte node ID. |
| `session.callsign()` | `()` | `string` | Returns the user's nickname, or `"RadioOperator"` if unset. |
| `session.await_input(max_len, callback)` | `(max_len: integer, callback: function)` | `nil` | Suspends execution until the user submits a keystroke, string, or form. |
| `session.load_app(app_name)` | `(app_name: string)` | `nil` | Switches the session to another enabled BBS app, triggering its `on_start(session)` hook. |
| `session.exec_app(app_name)` | `(app_name: string)` | `nil` | Alias for `session.load_app`. |
| `session.permissions()` | `()` | `table` | Returns an array of string permissions assigned to the current node. |
| `session.has_permission(perm)` | `(perm: string)` | `boolean` | Checks if the user holds a specific capability (`"admin"`, `"read"`, `"write"`, etc.). |
| `session.include(filename)` | `(filename: string)` | `any` | Loads and executes a Lua file within the active app directory. |
| `session.time()` | `()` | `integer` | Returns current Unix timestamp in seconds. |
| `session.date_str()` | `()` | `string` | Returns a day identifier (e.g. `"day-20690"`) useful for daily reset mechanics. |
| `session.close()` | `()` | `nil` | Gracefully closes the client session. |

---

### 5.3 Persistent Storage (`db`)

Bifrost provides a SQLite-backed key-value database engine partitioned by namespaces.

```lua
-- Single key get/set
db.set("my_app_players", user_id, { score = 1500, level = 3 })
local player = db.get("my_app_players", user_id)

-- List all keys in a table
local keys = db.keys("my_app_players")

-- Granular array / table storage
db.set("board_posts", "all", {
    ["1"] = { title = "First Post", author = "Alice" },
    ["2"] = { title = "Second Post", author = "Bob" }
})
local all_posts = db.get("board_posts", "all")

-- Delete a key
db.set("my_app_players", user_id, nil)
```

| Function | Arguments | Returns | Description |
| :--- | :--- | :--- | :--- |
| `db.get(table, [key])` | `(table: string, [key: string\|integer])` | `any` | Retrieves a stored Lua table/value. If `key` is omitted or `"all"`, returns all rows in the namespace. |
| `db.set(table, [key], val)` | `(table: string, [key: string\|integer], val: any)` | `nil` | Sets a value. Setting `nil` deletes the key. Setting `"all"` stores rows granularly. |
| `db.keys(table)` | `(table: string)` | `table` | Returns an array of all string keys in the specified namespace. |

---

### 5.4 System Logging (`log`)

Emits structured logs to the BBS supervisor and Heimdall web console:

```lua
log.info("Player " .. session.callsign() .. " entered the cantina.")
log.warn("Low inventory warning on commodity ID #4.")
log.error("Failed to parse game save state.")
log.debug("Packet buffer length: 42 bytes.")
```

---

### 5.5 HTTP & External Data (`http`)

Allows fetching data from whitelisted external APIs:

```lua
local data = http.get_json("https://api.open-meteo.com/v1/forecast?latitude=-41.28&longitude=174.77&current_weather=true")
if data then
    print("Temperature: " .. data.current_weather.temperature)
end
```

> **Security Note:** To protect BBS nodes on local intranets, URLs must begin with approved whitelist prefixes (e.g. `https://api.open-meteo.com/`).

---

## 6. Declarative UI: Forms & Menus

Bifrost applications use declarative UI components to minimize network chatter. Instead of transmitting individual keystrokes across the mesh for every character typed, clients edit forms locally and transmit a single compact payload upon submission.

### Declarative Menus (`assets/main_nav.csv`)

Define menus as CSV files in your app's `assets/` folder:

```csv
# form_id=10
# tag,id,label,col,row,key
boards,boards,MessageBoards,2,10,M
games,games,MiniDungeon,18,10,D
market,market,Marketplace,34,10,K
logout,logout,Logout,2,12,L
```

Render the menu and receive structured submissions in Lua:

```lua
term.render_menu("main_nav", {
    boards = true,
    games = true,
    market = true,
    logout = true
})
term.flush_form()

session.await_input(10, function(submission)
    local action = submission.submit
    if action == "boards" then
        session.load_app("messages")
    elseif action == "games" then
        session.load_app("minidungeon")
    end
end)
```

---

## 7. Asset Management & Caching

Static ANSI artwork and screens should be stored as `.ans` files under `assets/` and registered in `manifest.toml`:

```toml
[[assets]]
name = "logo"
path = "assets/logo.ans"
```

In Lua, call `term.render_asset("logo")`.

### ⚡ Why Use Assets?
* **Zero Airtime Transmission:** When a client already has the asset in its local cache (`.client_cache/<node_id>/`), the host only transmits a 3-byte opcode (`0xC5 <asset_id>`).
* **Opportunistic Multicast:** The host automatically broadcasts missing assets over unencrypted radio packets during idle airtime windows.

---

## 8. Session Lifecycle & Resumption Hooks

Bifrost supports **10-minute seamless session resumption**. If a user's radio temporarily drops offline and reconnects within 10 minutes, the host preserves their active Lua state and calls the `on_resume` hook.

```lua
local game = {}

function game.on_start(session)
    -- Initial application boot
    game.render_main_screen(session)
end

function game.on_resume(session)
    -- Re-render current screen without resetting player variables or state
    log.info("Resuming game session for " .. session.callsign())
    game.render_main_screen(session)
end

return game
```

---

## 9. Airtime & Performance Best Practices

1. **Always Batch Screen Updates Before Calling `term.flush()`:**
   * Avoid calling `term.flush()` after every `term.print()` line. Write all text and colors into the buffer, then flush once.
2. **Prefer Declarative Forms Over Keystroke Loops:**
   * Use `term.define_form()` and `session.await_input()` for text input and menu choices. This eliminates latency and airtime overhead.
3. **Use `term.render_table()` for Grids:**
   * `render_table` emits compact positioning and color opcodes optimized for dictionary and LZSS compression.
4. **Namespace Your Database Keys:**
   * Prefix database namespaces with your app name (e.g. `vt_players`, `vt_sectors`) to avoid key collisions with other BBS applications.
5. **Keep Line Lengths Under 80 Columns:**
   * Bifrost virtual terminals standardize on an **80x25 character grid**. Content exceeding column 80 will wrap to the next line.

---

*Happy Hacking on Bifrost MeshBBS!* 🚀
