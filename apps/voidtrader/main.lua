-- Void Trader: Interstellar Frontier Trading Strategy Door Game
-- An homage to classic BBS space trading games (e.g. TradeWars 2002)

local app = {}

local GRID_X = 30
local GRID_Y = 15
local GRID_Z = 5
local NUM_SECTORS = GRID_X * GRID_Y * GRID_Z -- 2,250 sectors

local START_X = 15
local START_Y = 8
local START_Z = 3

local MAX_TURNS = 120
local WARP_FUEL_COST = 3
local ORE_TO_FUEL_RATIO = 10

-- Ship Classes: name, holds_base, holds_max, max_fighters, max_shields, fuel_max, price
local SHIP_CLASSES = {
    { name = "Scout Sloop", holds_base = 20, holds_max = 35, max_fighters = 25, max_shields = 25, fuel_max = 30, price = 0 },
    { name = "Merchant Hauler", holds_base = 50, holds_max = 80, max_fighters = 60, max_shields = 60, fuel_max = 60, price = 4500 },
    { name = "Armored Freighter", holds_base = 100, holds_max = 150, max_fighters = 120, max_shields = 120, fuel_max = 120, price = 14000 },
    { name = "Dreadnought Cruiser", holds_base = 200, holds_max = 300, max_fighters = 250, max_shields = 250, fuel_max = 250, price = 42000 }
}

-- Navigation Computers: name, max_jumps, max_favorites, price
local NAV_COMPUTERS = {
    { name = "Mark I Basic Nav", max_jumps = 5, max_favorites = 3, price = 0 },
    { name = "Mark II Enhanced Nav", max_jumps = 10, max_favorites = 6, price = 1500 },
    { name = "Mark III Hyper-Nav", max_jumps = 20, max_favorites = 12, price = 4500 },
    { name = "Mark IV Quantum Core", max_jumps = 50, max_favorites = 30, price = 12000 }
}

-- Port Classes: [Ore, Org, Eqp] where 1 = Port BUYS (Player Sells), 0 = Port SELLS (Player Buys)
-- Class 0 is Stardock (Special Central Hub)
local PORT_CLASSES = {
    [1] = {1, 0, 0, name = "BBS (Ore Buy / Org Sell / Eqp Sell)"},
    [2] = {0, 1, 0, name = "BSB (Ore Sell / Org Buy / Eqp Sell)"},
    [3] = {0, 0, 1, name = "SBB (Ore Sell / Org Sell / Eqp Buy)"},
    [4] = {0, 1, 1, name = "SBB (Ore Sell / Org Buy / Eqp Buy)"},
    [5] = {1, 0, 1, name = "BSB (Ore Buy / Org Sell / Eqp Buy)"},
    [6] = {1, 1, 0, name = "BBS (Ore Buy / Org Buy / Eqp Sell)"},
    [7] = {1, 1, 1, name = "BBB (Import Station - Buys All)"},
    [8] = {0, 0, 0, name = "SSS (Industrial Hub - Sells All)"}
}

local BASE_PRICES = { ore = 15, org = 35, eqp = 80 }

-- ---------------------------------------------------------------------------
-- 3D GRID COORDINATES & TOPOLOGY
-- ---------------------------------------------------------------------------

local function to_coords(id)
    local z = math.floor((id - 1) / (GRID_X * GRID_Y)) + 1
    local rem = (id - 1) % (GRID_X * GRID_Y)
    local y = math.floor(rem / GRID_X) + 1
    local x = (rem % GRID_X) + 1
    return x, y, z
end

local function to_sector_id(x, y, z)
    if x < 1 or x > GRID_X or y < 1 or y > GRID_Y or z < 1 or z > GRID_Z then
        return nil
    end
    return (z - 1) * (GRID_X * GRID_Y) + (y - 1) * GRID_X + x
end

local START_SECTOR_ID = to_sector_id(START_X, START_Y, START_Z)

local function get_direction_name(from_id, to_id)
    local fx, fy, fz = to_coords(from_id)
    local tx, ty, tz = to_coords(to_id)
    if tx == fx + 1 then return "East", "E", "east" end
    if tx == fx - 1 then return "West", "W", "west" end
    if ty == fy - 1 then return "North", "N", "north" end
    if ty == fy + 1 then return "South", "S", "south" end
    if tz == fz + 1 then return "Up", "U", "up" end
    if tz == fz - 1 then return "Down", "D", "down" end
    return "Warp", "?", "warp"
end

-- ---------------------------------------------------------------------------
-- UNIVERSE & PLAYER DATABASE MANAGEMENT
-- ---------------------------------------------------------------------------

local function init_universe()
    local sectors = {}

    -- Step 1: Initialize sector nodes
    for i = 1, NUM_SECTORS do
        local port = nil
        if i == START_SECTOR_ID then
            -- Central Stardock at (15, 8, 3)
            port = { class = 0, name = "Alpha Stardock Prime", ore = 9999, org = 9999, eqp = 9999 }
        elseif math.random(1, 100) <= 60 then
            local p_class = math.random(1, 8)
            port = {
                class = p_class,
                name = "Port " .. string.char(64 + p_class) .. "-" .. i,
                ore = math.random(600, 3000),
                org = math.random(400, 2500),
                eqp = math.random(200, 1500)
            }
        end

        local hazard = nil
        local roll = math.random(1, 100)
        if i ~= START_SECTOR_ID then
            if roll <= 2 then
                hazard = "BLACK_HOLE"
            elseif roll <= 5 then
                hazard = "WORMHOLE"
            elseif roll <= 11 then
                hazard = "ASTEROID_FIELD"
            elseif roll <= 17 then
                hazard = "COSMIC_STORM"
            elseif roll <= 20 then
                hazard = "DERELICT_GRAVEYARD"
            end
        end

        sectors[i] = {
            id = i,
            warps = {},
            port = port,
            hazard = hazard,
            defense_fighters = 0,
            defense_owner = nil
        }
    end

    -- Helper to add a strictly bidirectional link between adjacent cells
    local function add_link(a, b)
        if not a or not b or a == b then return false end
        if #sectors[a].warps >= 4 or #sectors[b].warps >= 4 then return false end
        for _, w in ipairs(sectors[a].warps) do
            if w == b then return false end
        end
        table.insert(sectors[a].warps, b)
        table.insert(sectors[b].warps, a)
        return true
    end

    -- Step 2: Ensure spanning tree connectivity throughout the 15x30x5 volume
    for z = 1, GRID_Z do
        for y = 1, GRID_Y do
            for x = 1, GRID_X do
                local cur_id = to_sector_id(x, y, z)
                if x > 1 and math.random(1, 100) <= 70 then
                    add_link(cur_id, to_sector_id(x - 1, y, z))
                end
                if y > 1 and math.random(1, 100) <= 70 then
                    add_link(cur_id, to_sector_id(x, y - 1, z))
                end
                if z > 1 and math.random(1, 100) <= 60 then
                    add_link(cur_id, to_sector_id(x, y, z - 1))
                end
            end
        end
    end

    -- Step 3: Ensure every sector has at least 1 bidirectional link
    for i = 1, NUM_SECTORS do
        if #sectors[i].warps == 0 then
            local x, y, z = to_coords(i)
            local neighbors = {
                to_sector_id(x + 1, y, z), to_sector_id(x - 1, y, z),
                to_sector_id(x, y + 1, z), to_sector_id(x, y - 1, z),
                to_sector_id(x, y, z + 1), to_sector_id(x, y, z - 1)
            }
            for _ = 1, 6 do
                local n_id = neighbors[math.random(1, #neighbors)]
                if n_id and add_link(i, n_id) then break end
            end
        end
    end

    -- Ensure Start Sector has its adjacent warps
    local s_neighbors = {
        to_sector_id(START_X + 1, START_Y, START_Z),
        to_sector_id(START_X - 1, START_Y, START_Z),
        to_sector_id(START_X, START_Y + 1, START_Z),
        to_sector_id(START_X, START_Y - 1, START_Z)
    }
    for _, n in ipairs(s_neighbors) do
        if n then add_link(START_SECTOR_ID, n) end
    end

    db.set("vt_sectors", "all", sectors)
    return sectors
end

local function get_sectors()
    local s = db.get("vt_sectors", "all")
    if not s or type(s) ~= "table" or #s < NUM_SECTORS then
        s = init_universe()
    end
    return s
end

local function save_sectors(sectors)
    db.set("vt_sectors", "all", sectors)
end

local function init_player(session)
    local user = db.get("users", session.node_id()) or {}
    local nick = user.nickname or "Captain"
    return {
        nickname = nick,
        sector = START_SECTOR_ID,
        credits = 1200,
        bank = 0,
        turns = MAX_TURNS,
        ship_class = 1, -- Scout Sloop
        nav_level = 1,  -- Mark I Basic Nav
        fuel = 30,      -- Full starter tank
        holds = 20,
        ore = 0,
        ore_cost = 0,
        org = 0,
        org_cost = 0,
        eqp = 0,
        eqp_cost = 0,
        fighters = 15,
        shields = 15,
        kills = 0,
        trades = 0,
        favorites = {},
        plotted_course = {},
        explored = { [START_SECTOR_ID] = true }
    }
end

local function get_cargo_avg_cost(player, item_key)
    local qty = player[item_key] or 0
    local cost = player[item_key .. "_cost"] or 0
    if qty > 0 and cost > 0 then
        return math.floor((cost / qty) + 0.5)
    end
    return 0
end

local function get_player(session)
    local p = db.get("vt_players", session.node_id())
    if not p or type(p) ~= "table" then
        p = init_player(session)
        db.set("vt_players", session.node_id(), p)
    end

    local user = db.get("users", session.node_id()) or {}
    if user.nickname then p.nickname = user.nickname end

    if not p.turns or p.turns <= 0 then
        p.turns = MAX_TURNS
        db.set("vt_players", session.node_id(), p)
    end

    local ship_info = SHIP_CLASSES[p.ship_class or 1] or SHIP_CLASSES[1]
    if p.fuel == nil then p.fuel = ship_info.fuel_max end
    if p.nav_level == nil then p.nav_level = 1 end
    if p.favorites == nil then p.favorites = {} end
    if p.plotted_course == nil then p.plotted_course = {} end
    if p.explored == nil then p.explored = { [START_SECTOR_ID] = true } end
    p.explored[p.sector] = true

    return p
end

local function calc_net_worth(p)
    local ship_info = SHIP_CLASSES[p.ship_class or 1] or SHIP_CLASSES[1]
    local nav_info = NAV_COMPUTERS[p.nav_level or 1] or NAV_COMPUTERS[1]
    local cargo_val = (p.ore * BASE_PRICES.ore) + (p.org * BASE_PRICES.org) + (p.eqp * BASE_PRICES.eqp)
    local ship_val = ship_info.price + nav_info.price + ((p.holds - ship_info.holds_base) * 100)
    return p.credits + (p.bank or 0) + cargo_val + (p.fighters * 50) + (p.shields * 75) + ship_val
end

local function save_player(session, player)
    db.set("vt_players", session.node_id(), player)

    -- Update Galactic Hall of Fame Leaderboard
    local board = db.get("vt_leaderboard", "scores") or {}
    local my_id = session.node_id()
    local updated = false
    for i, entry in ipairs(board) do
        if entry.node_id == my_id then
            board[i] = {
                node_id = my_id,
                nickname = player.nickname or "Commander",
                net_worth = calc_net_worth(player),
                ship = SHIP_CLASSES[player.ship_class or 1].name,
                kills = player.kills or 0,
                sector = player.sector
            }
            updated = true
            break
        end
    end
    if not updated then
        table.insert(board, {
            node_id = my_id,
            nickname = player.nickname or "Commander",
            net_worth = calc_net_worth(player),
            ship = SHIP_CLASSES[player.ship_class or 1].name,
            kills = player.kills or 0,
            sector = player.sector
        })
    end

    table.sort(board, function(a, b) return (a.net_worth or 0) > (b.net_worth or 0) end)
    while #board > 15 do table.remove(board) end
    db.set("vt_leaderboard", "scores", board)
end

-- ---------------------------------------------------------------------------
-- SHORTEST PATH COURSE PLOTTER (BFS)
-- ---------------------------------------------------------------------------

local function find_shortest_path(sectors, start_sec, target_sec, max_depth)
    if start_sec == target_sec then return {} end
    local queue = { { start_sec } }
    local visited = { [start_sec] = true }

    while #queue > 0 do
        local path = table.remove(queue, 1)
        local current = path[#path]

        if #path - 1 < max_depth then
            local sec = sectors[current]
            for _, next_sec in ipairs((sec and sec.warps) or {}) do
                if next_sec == target_sec then
                    local res = {}
                    for i = 2, #path do table.insert(res, path[i]) end
                    table.insert(res, target_sec)
                    return res
                end
                if not visited[next_sec] then
                    visited[next_sec] = true
                    local new_path = {}
                    for _, node in ipairs(path) do table.insert(new_path, node) end
                    table.insert(new_path, next_sec)
                    table.insert(queue, new_path)
                end
            end
        end
    end
    return nil
end

local function is_favorite_sector(player, sec_id)
    for _, f in ipairs(player.favorites or {}) do
        if f == sec_id then return true end
    end
    return false
end

-- ---------------------------------------------------------------------------
-- MAIN NAVIGATION & SECTOR VIEW
-- ---------------------------------------------------------------------------

function app.on_start(session)
    local player = get_player(session)
    app.view_sector(session, player, "Welcome to the Void Frontier, " .. player.nickname .. "!")
end

function app.view_sector(session, player, msg)
    local sectors = get_sectors()
    local sec = sectors[player.sector] or sectors[START_SECTOR_ID]
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    player.explored = player.explored or {}
    player.explored[player.sector] = true
    for _, w in ipairs(sec.warps or {}) do
        player.explored[w] = true
    end

    local is_fav = is_favorite_sector(player, player.sector)
    local is_stranded = (player.fuel < WARP_FUEL_COST) and (player.ore <= 0)
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print(string.format("Sector: %-4d [%02d,%02d,%d]   Turns: %-3d   Credits: %-7d cr   Bank: %-7d cr", player.sector, cur_x, cur_y, cur_z, player.turns, player.credits, player.bank or 0))

    term.move_to(2, 7)
    term.set_color(11, 0)
    term.print(string.format("Ship: %-16s Fuel: %d/%-3d  Holds: %d/%-3d  Fgt: %-3d  Shd: %-3d", ship_info.name, player.fuel, ship_info.fuel_max, (player.ore + player.org + player.eqp), player.holds, player.fighters, player.shields))

    -- Directional Warp Links String
    term.move_to(2, 8)
    term.set_color(15, 0)
    local warp_str = ""
    local dir_map = {}
    for _, w in ipairs(sec.warps or {}) do
        local name, code, key = get_direction_name(player.sector, w)
        dir_map[key] = w
        warp_str = warp_str .. string.format("[%s] %s  ", code, name)
    end
    if warp_str == "" then warp_str = "(Dead End - No Vector)" end
    term.print("Available Warps: " .. warp_str)

    term.move_to(2, 9)
    local fav_marker = is_fav and " [* FAVORITE PORT]" or ""
    if player.sector == START_SECTOR_ID then
        term.set_color(10, 0)
        term.print("Facilities: [ Alpha Stardock Prime ] (Shipyard, Bank, Outfitter)" .. fav_marker)
    elseif sec.port then
        term.set_color(10, 0)
        local p_info = PORT_CLASSES[sec.port.class] or { name = "Trading Station" }
        term.print(string.format("Port: Class %d - %s%s", sec.port.class, p_info.name, fav_marker))
    else
        term.set_color(8, 0)
        term.print("Port: None in this sector (Deep Space)")
    end

    -- Display Hazard Warning
    if sec.hazard == "BLACK_HOLE" then
        term.move_to(2, 10)
        term.set_color(12, 0)
        term.print("HAZARD: BLACK HOLE SINGULARITY (Extreme Gravity Well!)")
    elseif sec.hazard == "WORMHOLE" then
        term.move_to(2, 10)
        term.set_color(13, 0)
        term.print("ANOMALY: UNSTABLE WORMHOLE RIFT (Trans-Galactic Conduit)")
    elseif sec.hazard == "ASTEROID_FIELD" then
        term.move_to(2, 10)
        term.set_color(14, 0)
        term.print("ANOMALY: DENSE ASTEROID BELT (Fuel Ore Deposits Available)")
    elseif sec.hazard == "COSMIC_STORM" then
        term.move_to(2, 10)
        term.set_color(11, 0)
        term.print("HAZARD: IONIZED COSMIC STORM (Sensor Static & Shield Drain)")
    elseif sec.hazard == "DERELICT_GRAVEYARD" then
        term.move_to(2, 10)
        term.set_color(15, 0)
        term.print("ANOMALY: DERELICT SHIP GRAVEYARD (Salvage Potential)")
    end

    -- Plotted Course summary line
    local next_course_hop = nil
    if player.plotted_course and #player.plotted_course > 0 then
        next_course_hop = player.plotted_course[1]
        term.move_to(2, 11)
        term.set_color(13, 0)
        local course_str = ""
        for idx, hop in ipairs(player.plotted_course) do
            if idx <= 5 then
                local _, code = get_direction_name(idx == 1 and player.sector or player.plotted_course[idx - 1], hop)
                course_str = course_str .. " -> [" .. code .. "]" .. hop
            end
        end
        if #player.plotted_course > 5 then course_str = course_str .. " ..." end
        term.print(string.format("Course Plotted (%d hops):%s", #player.plotted_course, course_str))
    end

    if is_stranded then
        term.move_to(2, 11)
        term.set_color(12, 0)
        term.print("STRANDED: Out of fuel & fuel ore! Send distress signal.")
    end

    term.move_to(2, 12)
    term.set_color(15, 0)
    term.print(msg or "")

    term.define_form(10)
    if is_stranded then
        term.add_submit_button("distress", 2, 14)
        term.add_submit_button("chart", 14, 14)
        term.add_submit_button("scan", 24, 14)
        term.add_submit_button("status", 32, 14)
        term.add_submit_button("ranks", 42, 14)
        term.add_submit_button("exit", 52, 14)
    else
        -- Directional Warp Buttons on First Row
        local col_pos = 2
        if dir_map["north"] then
            term.add_submit_button("warp_north", col_pos, 14)
            col_pos = col_pos + 8
        end
        if dir_map["south"] then
            term.add_submit_button("warp_south", col_pos, 14)
            col_pos = col_pos + 8
        end
        if dir_map["east"] then
            term.add_submit_button("warp_east", col_pos, 14)
            col_pos = col_pos + 8
        end
        if dir_map["west"] then
            term.add_submit_button("warp_west", col_pos, 14)
            col_pos = col_pos + 8
        end
        if dir_map["up"] then
            term.add_submit_button("warp_up", col_pos, 14)
            col_pos = col_pos + 6
        end
        if dir_map["down"] then
            term.add_submit_button("warp_down", col_pos, 14)
            col_pos = col_pos + 8
        end

        if next_course_hop then
            term.add_submit_button("autowarp", col_pos, 14)
            col_pos = col_pos + 11
        end

        term.add_submit_button("chart", col_pos, 14)

        -- Row 16: Operations & Facilities
        local col2 = 2
        if player.sector == START_SECTOR_ID then
            term.add_submit_button("stardock", col2, 16)
            col2 = col2 + 11
        elseif sec.port then
            term.add_submit_button("dock", col2, 16)
            col2 = col2 + 8
            if is_fav then
                term.add_submit_button("unfav", col2, 16)
                col2 = col2 + 8
            else
                term.add_submit_button("fav", col2, 16)
                col2 = col2 + 6
            end
        end

        if sec.hazard == "WORMHOLE" then
            term.add_submit_button("wormhole", col2, 16)
            col2 = col2 + 11
        elseif sec.hazard == "ASTEROID_FIELD" then
            term.add_submit_button("mine_ore", col2, 16)
            col2 = col2 + 11
        elseif sec.hazard == "DERELICT_GRAVEYARD" then
            term.add_submit_button("salvage", col2, 16)
            col2 = col2 + 10
        elseif sec.hazard == "BLACK_HOLE" then
            term.add_submit_button("slingshot", col2, 16)
            col2 = col2 + 12
        end

        if player.ore > 0 and player.fuel < ship_info.fuel_max then
            term.add_submit_button("refuel", col2, 16)
            col2 = col2 + 9
        end

        term.add_submit_button("plot", col2, 16)
        col2 = col2 + 7
        term.add_submit_button("scan", col2, 16)
        col2 = col2 + 7
        term.add_submit_button("status", col2, 16)
        col2 = col2 + 9
        term.add_submit_button("ranks", col2, 16)
        col2 = col2 + 8
        term.add_submit_button("exit", col2, 16)
    end
    term.flush_form()

    session.await_input(10, function(sub)
        if type(sub) == "string" then app.view_sector(session, player, "") return end
        local act = sub.submit

        if act == "warp_north" and dir_map["north"] then
            app.perform_warp_to(session, player, dir_map["north"])
        elseif act == "warp_south" and dir_map["south"] then
            app.perform_warp_to(session, player, dir_map["south"])
        elseif act == "warp_east" and dir_map["east"] then
            app.perform_warp_to(session, player, dir_map["east"])
        elseif act == "warp_west" and dir_map["west"] then
            app.perform_warp_to(session, player, dir_map["west"])
        elseif act == "warp_up" and dir_map["up"] then
            app.perform_warp_to(session, player, dir_map["up"])
        elseif act == "warp_down" and dir_map["down"] then
            app.perform_warp_to(session, player, dir_map["down"])
        elseif act == "autowarp" then
            app.execute_autowarp(session, player)
        elseif act == "plot" then
            app.plot_menu(session, player)
        elseif act == "chart" then
            local _, _, z = to_coords(player.sector)
            app.starchart_view(session, player, z)
        elseif act == "dock" then
            app.port_menu(session, player)
        elseif act == "stardock" then
            app.stardock_menu(session, player)
        elseif act == "fav" then
            app.add_favorite(session, player)
        elseif act == "unfav" then
            app.remove_favorite(session, player)
        elseif act == "wormhole" then
            app.traverse_wormhole(session, player)
        elseif act == "mine_ore" then
            app.mine_asteroid_field(session, player)
        elseif act == "salvage" then
            app.salvage_derelict_graveyard(session, player)
        elseif act == "slingshot" then
            app.black_hole_slingshot(session, player)
        elseif act == "refuel" then
            app.refuel_from_ore(session, player)
        elseif act == "distress" then
            app.distress_beacon(session, player)
        elseif act == "scan" then
            app.scan_sector(session, player)
        elseif act == "status" then
            app.view_status(session, player)
        elseif act == "ranks" then
            app.view_leaderboard(session, player)
        elseif act == "exit" then
            save_player(session, player)
            session.load_app("main_menu")
        else
            app.view_sector(session, player, "")
        end
    end)
end

-- ---------------------------------------------------------------------------
-- 3D STARCHART VIEWER (15x30x5 COMPACT SINGLE-CHAR GRID)
-- ---------------------------------------------------------------------------

function app.starchart_view(session, player, plane_z)
    plane_z = math.max(1, math.min(GRID_Z, plane_z or START_Z))
    local sectors = get_sectors()
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0)
    term.print(string.format("=== STARCHART: PLANE Z=%d/%d === (Position: Sec %d [%02d,%02d,%d])", plane_z, GRID_Z, player.sector, cur_x, cur_y, cur_z))

    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print("Legend: @=You S=Stardock P=Port A=Asteroid W=Wormhole B=BlackHole +=Space .=Fog")

    -- Top border of the 30-wide box
    term.move_to(4, 4)
    term.set_color(7, 0)
    term.print("+------------------------------+")

    player.explored = player.explored or {}

    for row_y = 1, GRID_Y do
        term.move_to(4, 4 + row_y)
        term.set_color(7, 0)
        term.print("|")

        for col_x = 1, GRID_X do
            local sec_id = to_sector_id(col_x, row_y, plane_z)
            local is_here = (player.sector == sec_id)
            local is_exp = player.explored[sec_id]

            local ch = "."
            local col = 8 -- Grey fog

            if is_here then
                ch = "@"
                col = 14 -- Bright Yellow
            elseif is_exp then
                local s_data = sectors[sec_id]
                if sec_id == START_SECTOR_ID then
                    ch = "S"
                    col = 10 -- Green Stardock
                elseif s_data and s_data.hazard == "BLACK_HOLE" then
                    ch = "B"
                    col = 12 -- Red
                elseif s_data and s_data.hazard == "WORMHOLE" then
                    ch = "W"
                    col = 13 -- Magenta
                elseif s_data and s_data.hazard == "ASTEROID_FIELD" then
                    ch = "A"
                    col = 14 -- Yellow
                elseif s_data and s_data.hazard == "COSMIC_STORM" then
                    ch = "C"
                    col = 11 -- Cyan
                elseif s_data and s_data.hazard == "DERELICT_GRAVEYARD" then
                    ch = "D"
                    col = 15 -- White
                elseif s_data and s_data.port then
                    ch = "P"
                    col = 10 -- Green
                else
                    ch = "+"
                    col = 7 -- Light Grey space
                end
            end

            term.set_color(col, 0)
            term.print(ch)
        end

        term.set_color(7, 0)
        term.print("|")
    end

    -- Bottom border of 30-wide box
    term.move_to(4, 5 + GRID_Y)
    term.set_color(7, 0)
    term.print("+------------------------------+")

    term.define_form(35)
    if plane_z > 1 then
        term.add_submit_button("plane_down", 4, 22)
    end
    if plane_z < GRID_Z then
        term.add_submit_button("plane_up", 18, 22)
    end
    term.add_submit_button("back", 32, 22)
    term.flush_form()

    session.await_input(35, function(sub)
        if type(sub) == "string" then app.starchart_view(session, player, plane_z) return end
        local act = sub.submit
        if act == "back" then
            app.view_sector(session, player, "")
            return
        end

        if act == "plane_down" then
            app.starchart_view(session, player, plane_z - 1)
        elseif act == "plane_up" then
            app.starchart_view(session, player, plane_z + 1)
        else
            app.starchart_view(session, player, plane_z)
        end
    end)
end

-- ---------------------------------------------------------------------------
-- HAZARD INTERACTIONS: WORMHOLE, MINING, SALVAGE, SLINGSHOT
-- ---------------------------------------------------------------------------

function app.traverse_wormhole(session, player)
    if player.turns <= 0 then
        app.view_sector(session, player, "Out of turns to traverse wormhole.")
        return
    end

    player.turns = player.turns - 1

    local dest = math.random(1, NUM_SECTORS)
    while dest == player.sector do dest = math.random(1, NUM_SECTORS) end

    player.sector = dest
    player.plotted_course = {}

    local dmg_msg = ""
    if math.random(1, 100) <= 25 then
        local dmg = math.random(2, 5)
        if player.shields > 0 then
            player.shields = math.max(0, player.shields - dmg)
            dmg_msg = string.format(" (Turbulence: Shields absorbed %d dmg)", dmg)
        end
    end

    save_player(session, player)
    app.view_sector(session, player, string.format("Traversed wormhole rift! Spacetime collapsed and ejected vessel into Sector %d.%s", dest, dmg_msg))
end

function app.mine_asteroid_field(session, player)
    local holds_used = player.ore + player.org + player.eqp
    local free_holds = player.holds - holds_used
    if free_holds <= 0 then
        app.view_sector(session, player, "Cargo bays full! Upgrade holds at Stardock or sell cargo.")
        return
    end

    if player.turns <= 0 then
        app.view_sector(session, player, "Out of turns to operate mining lasers.")
        return
    end

    player.turns = player.turns - 1

    local roll = math.random(1, 100)
    if roll <= 65 then
        local yield = math.min(free_holds, math.random(6, 18))
        player.ore = player.ore + yield
        save_player(session, player)
        app.view_sector(session, player, string.format("Mining lasers extracted %d units of raw Fuel Ore! (Free holds: %d)", yield, free_holds - yield))
    elseif roll <= 85 then
        local yield = math.min(free_holds, math.random(3, 8))
        player.ore = player.ore + yield
        local dmg = math.random(2, 6)
        if player.shields > 0 then
            player.shields = math.max(0, player.shields - dmg)
        else
            player.fighters = math.max(0, player.fighters - 1)
        end
        save_player(session, player)
        app.view_sector(session, player, string.format("Mined %d Fuel Ore, but micrometeorites struck ship (%d shield dmg)!", yield, dmg))
    else
        app.pirate_encounter(session, player, math.random(8, 20))
    end
end

function app.salvage_derelict_graveyard(session, player)
    if player.turns <= 0 then
        app.view_sector(session, player, "Out of turns to conduct salvage operations.")
        return
    end

    player.turns = player.turns - 1

    local roll = math.random(1, 100)
    if roll <= 60 then
        local creds = math.random(250, 900)
        player.credits = player.credits + creds
        save_player(session, player)
        app.view_sector(session, player, string.format("Salvaged intact credit vaults from drifting hulks! Found %d cr.", creds))
    elseif roll <= 85 then
        local holds_used = player.ore + player.org + player.eqp
        local free_holds = player.holds - holds_used
        local eqp_found = math.min(free_holds, math.random(2, 6))
        player.eqp = player.eqp + eqp_found
        save_player(session, player)
        app.view_sector(session, player, string.format("Salvaged %d units of high-tech Equipment components from wreckage!", eqp_found))
    else
        app.pirate_encounter(session, player, math.random(10, 22))
    end
end

function app.black_hole_slingshot(session, player)
    local sectors = get_sectors()
    local sec = sectors[player.sector]
    local warps = (sec and sec.warps) or {}
    if #warps == 0 then
        app.view_sector(session, player, "No exit vectors from singularity!")
        return
    end

    if player.shields <= 0 and player.fighters <= 0 then
        app.player_death(session, player, "Vessel pulled past event horizon and crushed by singularity.")
        return
    end

    local next1 = warps[math.random(1, #warps)]
    local next2 = next1
    local sec2 = sectors[next1]
    if sec2 and #sec2.warps > 0 then
        next2 = sec2.warps[math.random(1, #sec2.warps)]
    end

    player.sector = next2
    local dmg = math.random(3, 8)
    if player.shields > 0 then
        player.shields = math.max(0, player.shields - dmg)
    end
    player.plotted_course = {}
    save_player(session, player)

    app.view_sector(session, player, string.format("Executed gravitational slingshot! Accelerated past event horizon into Sector %d (Shields absorbed %d dmg).", next2, dmg))
end

-- ---------------------------------------------------------------------------
-- FAVORITES & COURSE PLOTTING
-- ---------------------------------------------------------------------------

function app.add_favorite(session, player)
    local nav_info = NAV_COMPUTERS[player.nav_level or 1] or NAV_COMPUTERS[1]
    player.favorites = player.favorites or {}
    if #player.favorites >= nav_info.max_favorites then
        app.view_sector(session, player, string.format("Nav memory full! Max %d favorites (Upgrade at Stardock).", nav_info.max_favorites))
        return
    end
    for _, f in ipairs(player.favorites) do
        if f == player.sector then
            app.view_sector(session, player, "Sector already in favorites.")
            return
        end
    end
    table.insert(player.favorites, player.sector)
    save_player(session, player)
    app.view_sector(session, player, "Added Sector " .. player.sector .. " to Nav Favorites!")
end

function app.remove_favorite(session, player)
    player.favorites = player.favorites or {}
    for i, f in ipairs(player.favorites) do
        if f == player.sector then
            table.remove(player.favorites, i)
            save_player(session, player)
            app.view_sector(session, player, "Removed Sector " .. player.sector .. " from Favorites.")
            return
        end
    end
    app.view_sector(session, player, "")
end

function app.plot_menu(session, player)
    local sectors = get_sectors()
    local nav_info = NAV_COMPUTERS[player.nav_level or 1] or NAV_COMPUTERS[1]
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print(string.format("=== HYPERSPACE COURSE PLOTTER (%s) ===", nav_info.name))
    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print(string.format("Position: Sec %d [%02d,%02d,%d] | Max Jumps Plottable: %d", player.sector, cur_x, cur_y, cur_z, nav_info.max_jumps))

    term.move_to(2, 8)
    term.set_color(15, 0)
    term.print("Favorite Starports:")
    if player.favorites and #player.favorites > 0 then
        for idx, fav_sec in ipairs(player.favorites) do
            if idx <= 4 then
                local sec = sectors[fav_sec]
                local fx, fy, fz = to_coords(fav_sec)
                local port_name = (sec and sec.port and sec.port.name) or "Deep Space"
                term.move_to(4, 8 + idx)
                term.set_color(10, 0)
                term.print(string.format("[%d] Sector %-4d [%02d,%02d,%d] - %s", idx, fav_sec, fx, fy, fz, port_name))
            end
        end
    else
        term.move_to(4, 9)
        term.set_color(8, 0)
        term.print("No favorite ports saved. (Press [fav] at any starport)")
    end

    term.define_form(25)
    term.move_to(2, 13)
    term.set_color(15, 0)
    term.print("Enter Target Sector (1 - " .. NUM_SECTORS .. "): ")
    term.add_input_field("target", 34, 13, 5, "")

    term.add_submit_button("plot_course", 2, 15)
    term.add_submit_button("clear_course", 16, 15)
    term.add_submit_button("back", 32, 15)
    term.flush_form()

    session.await_input(25, function(sub)
        if type(sub) == "string" then app.plot_menu(session, player) return end
        local act = sub.submit
        if act == "back" then app.view_sector(session, player, "") return end

        if act == "clear_course" then
            player.plotted_course = {}
            save_player(session, player)
            app.view_sector(session, player, "Plotted course cleared.")
            return
        end

        local target = tonumber(sub.target)
        if not target or target < 1 or target > NUM_SECTORS then
            app.view_sector(session, player, "Invalid target sector coordinates.")
            return
        end

        if target == player.sector then
            app.view_sector(session, player, "You are already in Sector " .. target .. "!")
            return
        end

        local path = find_shortest_path(sectors, player.sector, target, nav_info.max_jumps)
        if not path or #path == 0 then
            app.view_sector(session, player, string.format("No route found within %d jumps of current Nav Computer.", nav_info.max_jumps))
            return
        end

        player.plotted_course = path
        save_player(session, player)
        app.view_sector(session, player, string.format("Course plotted to Sector %d (%d jumps). Use [autowarp] to travel.", target, #path))
    end)
end

function app.execute_autowarp(session, player)
    if not player.plotted_course or #player.plotted_course == 0 then
        app.view_sector(session, player, "No course plotted.")
        return
    end

    local dest = player.plotted_course[1]
    local sectors = get_sectors()
    local current_sec = sectors[player.sector]

    local valid = false
    for _, w in ipairs((current_sec and current_sec.warps) or {}) do
        if w == dest then valid = true break end
    end

    if not valid then
        player.plotted_course = {}
        save_player(session, player)
        app.view_sector(session, player, "Deviated from plotted route. Course reset.")
        return
    end

    table.remove(player.plotted_course, 1)
    app.perform_warp_to(session, player, dest)
end

-- ---------------------------------------------------------------------------
-- FUEL REFINING & DISTRESS BEACON
-- ---------------------------------------------------------------------------

function app.refuel_from_ore(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]
    local needed = ship_info.fuel_max - player.fuel
    if needed <= 0 then
        app.view_sector(session, player, "Fuel tank is already at maximum capacity.")
        return
    end

    local ore_needed = math.ceil(needed / ORE_TO_FUEL_RATIO)
    local ore_to_use = math.min(player.ore, ore_needed)

    if ore_to_use <= 0 then
        app.view_sector(session, player, "No Fuel Ore in cargo holds to refine.")
        return
    end

    local fuel_gain = math.min(needed, ore_to_use * ORE_TO_FUEL_RATIO)
    player.ore = player.ore - ore_to_use
    player.fuel = player.fuel + fuel_gain

    if player.ore <= 0 then
        player.ore = 0
        player.ore_cost = 0
    else
        local avg_cost = get_cargo_avg_cost(player, "ore")
        player.ore_cost = player.ore * avg_cost
    end

    save_player(session, player)
    app.view_sector(session, player, string.format("Refined %d Fuel Ore into %d units of fuel! (Tank: %d/%d)", ore_to_use, fuel_gain, player.fuel, ship_info.fuel_max))
end

function app.distress_beacon(session, player)
    if player.turns <= 0 then
        app.view_sector(session, player, "Out of turns to transmit distress beacon.")
        return
    end

    player.turns = player.turns - 1
    save_player(session, player)

    local roll = math.random(1, 100)
    if roll <= 35 then
        local fuel_given = 15
        player.fuel = player.fuel + fuel_given
        save_player(session, player)
        app.view_sector(session, player, string.format("Alpha Patrol Corvette answered distress call! Transfused %d free fuel.", fuel_given))
    elseif roll <= 70 then
        local price = math.random(200, 600)
        if player.credits >= price then
            player.credits = player.credits - price
            player.fuel = player.fuel + 15
            save_player(session, player)
            app.view_sector(session, player, string.format("Scavenger hauler answered beacon! Sold you 15 fuel for %d cr.", price))
        else
            app.view_sector(session, player, "Scavenger answered beacon, but you couldn't afford their price. They left.")
        end
    else
        app.pirate_encounter(session, player, math.random(10, 25))
    end
end

-- ---------------------------------------------------------------------------
-- NAVIGATION & WARPING
-- ---------------------------------------------------------------------------

function app.perform_warp_to(session, player, dest)
    local sectors = get_sectors()

    if player.turns <= 0 then
        app.view_sector(session, player, "Hyperspace engine offline: Out of turns!")
        return
    end

    if player.fuel < WARP_FUEL_COST then
        if player.ore > 0 then
            app.refuel_from_ore(session, player)
            return
        else
            app.view_sector(session, player, "Hyperspace jump failed: Out of fuel! Send distress signal.")
            return
        end
    end

    player.sector = dest
    player.turns = player.turns - 1
    player.fuel = player.fuel - WARP_FUEL_COST
    save_player(session, player)

    local dest_sec = sectors[dest]

    -- Storm interference
    if dest_sec.hazard == "COSMIC_STORM" and player.shields > 0 then
        local dmg = math.random(1, 3)
        player.shields = math.max(0, player.shields - dmg)
        save_player(session, player)
    end

    -- Random Encounters (Pirate ambush or Derelict salvage)
    local roll = math.random(1, 100)
    if dest ~= START_SECTOR_ID and roll <= 16 then
        app.pirate_encounter(session, player, math.random(8, 25))
    elseif dest ~= START_SECTOR_ID and roll <= 26 then
        app.derelict_salvage(session, player)
    else
        local _, code, _ = get_direction_name(dest, player.sector)
        app.view_sector(session, player, string.format("Warped into Sector %d [%s vector].", dest, code))
    end
end

-- ---------------------------------------------------------------------------
-- COMBAT & SPACE ENCOUNTERS
-- ---------------------------------------------------------------------------

function app.pirate_encounter(session, player, enemy_fighters)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(12, 0)
    term.print("RED ALERT: Void Marauder Corsair dropping out of warp!")

    term.move_to(2, 8)
    term.set_color(7, 0)
    term.print(string.format("Enemy Combat Drones: %-3d", enemy_fighters))
    term.move_to(2, 9)
    term.print(string.format("Your Fleet: %d Fighters | %d Deflector Shields", player.fighters, player.shields))

    term.define_form(30)
    term.add_submit_button("attack", 2, 12)
    term.add_submit_button("shields_up", 14, 12)
    term.add_submit_button("hyper_flee", 28, 12)
    term.flush_form()

    session.await_input(30, function(sub)
        if type(sub) == "string" then app.pirate_encounter(session, player, enemy_fighters) return end
        local act = sub.submit

        if act == "attack" then
            local p_dmg = math.random(2, math.max(3, math.floor(player.fighters * 0.8)))
            local e_dmg = math.random(2, math.max(3, math.floor(enemy_fighters * 0.7)))

            if player.shields > 0 then
                local abs = math.min(player.shields, e_dmg)
                player.shields = player.shields - abs
                e_dmg = e_dmg - abs
            end
            player.fighters = math.max(0, player.fighters - e_dmg)
            enemy_fighters = math.max(0, enemy_fighters - p_dmg)

            if enemy_fighters <= 0 then
                local bounty = math.random(350, 1200)
                player.credits = player.credits + bounty
                player.kills = (player.kills or 0) + 1
                save_player(session, player)
                app.view_sector(session, player, "Victory! Corsair destroyed. Salvaged bounty: " .. bounty .. " cr.")
            elseif player.fighters <= 0 and player.shields <= 0 then
                app.player_death(session, player, "Your vessel was torn apart by Void Marauders.")
            else
                app.pirate_encounter(session, player, enemy_fighters)
            end
        elseif act == "shields_up" then
            local e_dmg = math.random(1, math.max(2, math.floor(enemy_fighters * 0.4)))
            if player.shields > 0 then
                player.shields = math.max(0, player.shields - e_dmg)
            else
                player.fighters = math.max(0, player.fighters - e_dmg)
            end

            if player.fighters <= 0 then
                app.player_death(session, player, "Shields collapsed and vessel destroyed.")
            else
                app.pirate_encounter(session, player, enemy_fighters)
            end
        elseif act == "hyper_flee" then
            if math.random(1, 100) <= 60 and player.fuel >= WARP_FUEL_COST then
                player.turns = math.max(0, player.turns - 1)
                player.fuel = math.max(0, player.fuel - WARP_FUEL_COST)
                save_player(session, player)
                app.view_sector(session, player, "Engaged emergency hyperdrive jump! Escaped to safety.")
            else
                local dmg = math.random(2, 6)
                if player.shields > 0 then
                    player.shields = math.max(0, player.shields - dmg)
                else
                    player.fighters = math.max(0, player.fighters - dmg)
                end

                if player.fighters <= 0 then
                    app.player_death(session, player, "Destroyed while attempting hyperdrive escape.")
                else
                    app.pirate_encounter(session, player, enemy_fighters)
                end
            end
        end
    end)
end

function app.derelict_salvage(session, player)
    local credits_found = math.random(150, 600)
    local ore_found = math.random(5, 15)
    local free_holds = player.holds - (player.ore + player.org + player.eqp)
    local ore_taken = math.min(free_holds, ore_found)

    player.credits = player.credits + credits_found
    player.ore = player.ore + ore_taken
    save_player(session, player)

    app.view_sector(session, player, string.format("Discovered a drifting derelict freighter! Salvaged %d cr and %d Fuel Ore.", credits_found, ore_taken))
end

function app.player_death(session, player, cause)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(12, 0)
    term.print("==========================================================")
    term.move_to(2, 7)
    term.print("                   VESSEL DESTROYED                       ")
    term.move_to(2, 8)
    term.print("==========================================================")
    term.move_to(2, 10)
    term.set_color(15, 0)
    term.print(cause)
    term.move_to(2, 12)
    term.set_color(14, 0)
    term.print("Escape pod retrieved by Alpha Stardock rescue team.")
    term.move_to(2, 13)
    term.print("Your bank account credits remained secure.")

    local saved_bank = player.bank or 0
    local p = init_player(session)
    p.bank = saved_bank
    save_player(session, p)

    term.define_form(40)
    term.add_submit_button("respawn", 2, 15)
    term.flush_form()

    session.await_input(40, function() app.view_sector(session, p, "Respawned at Alpha Stardock Prime.") end)
end

-- ---------------------------------------------------------------------------
-- COMMODITY TRADING AT STARPORTS (WITH 1 CREDIT/UNIT REFUELING)
-- ---------------------------------------------------------------------------

function app.port_menu(session, player)
    local sectors = get_sectors()
    local sec = sectors[player.sector]
    if not sec or not sec.port then
        app.view_sector(session, player, "No trading post in this sector.")
        return
    end

    local port = sec.port
    local p_rules = PORT_CLASSES[port.class] or {1, 0, 0}
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print(string.format("=== %s (Class %d) ===", port.name or "Commerce Post", port.class))

    term.move_to(2, 7)
    term.set_color(14, 0)
    term.print("Commodity    Port Supply   Cargo (Avg Paid)   Action / Price   Margin / Diff")
    term.move_to(2, 8)
    term.print("---------    -----------   ----------------   --------------   -------------")

    local function port_item(y, key, name, port_amt, pl_amt, rule, base_p)
        term.move_to(2, y)
        local is_station_buying = (rule == 1)
        local price = is_station_buying and math.floor(base_p * 0.9) or math.floor(base_p * 1.15)
        local act_str = is_station_buying and string.format("BUY  @ %-3d cr", price) or string.format("SELL @ %-3d cr", price)

        local cargo_str = ""
        local avg_p = get_cargo_avg_cost(player, key)
        if pl_amt > 0 then
            cargo_str = string.format("%d (%d cr)", pl_amt, avg_p)
        else
            cargo_str = "0 (-)"
        end

        local margin_str = "---"
        local margin_color = 15
        if is_station_buying and pl_amt > 0 then
            local diff = price - avg_p
            local pct = avg_p > 0 and math.floor((diff / avg_p) * 100) or 0
            if diff >= 0 then
                margin_str = string.format("+%d cr (+%d%%)", diff, pct)
                margin_color = 10
            else
                margin_str = string.format("%d cr (%d%%)", diff, pct)
                margin_color = 12
            end
        elseif not is_station_buying and pl_amt > 0 then
            margin_str = string.format("Hold: %d cr", avg_p)
            margin_color = 11
        end

        term.set_color(15, 0)
        term.print(string.format("%-11s  %-11d   %-16s   %-14s   ", name, port_amt, cargo_str, act_str))
        term.set_color(margin_color, 0)
        term.print(margin_str)

        return price
    end

    local pr_ore = port_item(9, "ore", "Fuel Ore", port.ore, player.ore, p_rules[1], BASE_PRICES.ore)
    local pr_org = port_item(10, "org", "Organics", port.org, player.org, p_rules[2], BASE_PRICES.org)
    local pr_eqp = port_item(11, "eqp", "Equipment", port.eqp, player.eqp, p_rules[3], BASE_PRICES.eqp)

    term.move_to(2, 13)
    term.set_color(15, 0)
    local holds_used = player.ore + player.org + player.eqp
    term.print(string.format("Cash: %-8d cr   Holds: %d / %d   Fuel: %d / %d (Refuel: 1 cr/unit)", player.credits, (player.holds - holds_used), player.holds, player.fuel, ship_info.fuel_max))

    term.define_form(50)
    term.add_submit_button("trade_ore", 2, 15)
    term.add_submit_button("trade_org", 16, 15)
    term.add_submit_button("trade_eqp", 30, 15)
    if player.fuel < ship_info.fuel_max then
        term.add_submit_button("refuel_tank", 44, 15)
        term.add_submit_button("depart", 60, 15)
    else
        term.add_submit_button("depart", 44, 15)
    end
    term.flush_form()

    session.await_input(50, function(sub)
        if type(sub) == "string" then app.port_menu(session, player) return end
        local act = sub.submit

        if act == "depart" then
            app.view_sector(session, player, "Departed starport.")
            return
        end

        if act == "refuel_tank" then
            local missing = ship_info.fuel_max - player.fuel
            local max_afford = player.credits
            local to_add = math.min(missing, max_afford)
            if to_add > 0 then
                player.credits = player.credits - to_add
                player.fuel = player.fuel + to_add
                save_player(session, player)
            end
            app.port_menu(session, player)
            return
        end

        if act == "trade_ore" then
            app.trade_quantity_prompt(session, player, "ore", "Fuel Ore", p_rules[1], pr_ore, port)
        elseif act == "trade_org" then
            app.trade_quantity_prompt(session, player, "org", "Organics", p_rules[2], pr_org, port)
        elseif act == "trade_eqp" then
            app.trade_quantity_prompt(session, player, "eqp", "Equipment", p_rules[3], pr_eqp, port)
        end
    end)
end

function app.trade_quantity_prompt(session, player, item_key, item_name, rule, price, port)
    local sectors = get_sectors()
    local free_holds = player.holds - (player.ore + player.org + player.eqp)
    local pl_amt = player[item_key] or 0
    local port_amt = port[item_key] or 0
    local avg_paid = get_cargo_avg_cost(player, item_key)

    local is_station_buying = (rule == 1)
    local max_qty = 0

    if is_station_buying then
        max_qty = pl_amt
    else
        local max_afford = math.floor(player.credits / price)
        max_qty = math.min(free_holds, max_afford, port_amt)
    end

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print("=== COMMODITY TRANSACTION: " .. string.upper(item_name) .. " ===")

    term.move_to(2, 7)
    term.set_color(15, 0)
    if is_station_buying then
        term.print(string.format("Mode: Station is BUYING from you (You SELL) @ %d cr/unit", price))
        term.move_to(2, 8)
        term.print(string.format("In Cargo: %d units (Avg Paid: %d cr/unit) | Max Tradable: %d units", pl_amt, avg_paid, max_qty))

        term.move_to(2, 9)
        local diff = price - avg_paid
        local pct = avg_paid > 0 and math.floor((diff / avg_paid) * 100) or 0
        if diff >= 0 then
            term.set_color(10, 0)
            term.print(string.format("Profit Margin: +%d cr/unit (+%d%%) [ PROFITABLE TRANSACTION ]", diff, pct))
        else
            term.set_color(12, 0)
            term.print(string.format("Loss Warning:  %d cr/unit (%d%%) [ SELLING AT A LOSS ]", diff, pct))
        end
    else
        term.print(string.format("Mode: Station is SELLING to you (You BUY) @ %d cr/unit", price))
        term.move_to(2, 8)
        term.print(string.format("Port Stock: %d | Empty Holds: %d | Max Afford: %d", port_amt, free_holds, math.floor(player.credits / price)))
        term.move_to(2, 9)
        if pl_amt > 0 then
            term.print(string.format("Current Cargo: %d units (Avg Paid: %d cr/unit) | Max Tradable: %d", pl_amt, avg_paid, max_qty))
        else
            term.print(string.format("Maximum Tradable: %d units", max_qty))
        end
    end

    term.move_to(2, 11)
    term.set_color(14, 0)
    term.print(string.format("Cash on Hand: %d cr   |   Holds: %d / %d", player.credits, (player.holds - free_holds), player.holds))

    term.define_form(55)
    term.move_to(2, 13)
    term.set_color(15, 0)
    term.print("Trade Quantity: ")
    term.add_input_field("qty", 18, 13, 6, tostring(max_qty))

    term.add_submit_button("trade", 2, 15)
    term.add_submit_button("trade_all", 14, 15)
    term.add_submit_button("cancel", 28, 15)
    term.flush_form()

    session.await_input(55, function(sub)
        if type(sub) == "string" then
            app.trade_quantity_prompt(session, player, item_key, item_name, rule, price, port)
            return
        end

        local act = sub.submit
        if act == "cancel" then
            app.port_menu(session, player)
            return
        end

        local qty_to_trade = 0
        if act == "trade_all" then
            qty_to_trade = max_qty
        else
            local input_num = tonumber(sub.qty)
            if not input_num or input_num < 0 then
                app.port_menu(session, player)
                return
            end
            qty_to_trade = math.min(input_num, max_qty)
        end

        if qty_to_trade <= 0 then
            app.port_menu(session, player)
            return
        end

        if is_station_buying then
            local total_val = qty_to_trade * price
            player.credits = player.credits + total_val

            local current_qty = player[item_key] or 0
            local current_cost = player[item_key .. "_cost"] or 0
            local avg_cost_per_unit = current_qty > 0 and (current_cost / current_qty) or 0

            player[item_key] = current_qty - qty_to_trade
            if player[item_key] <= 0 then
                player[item_key] = 0
                player[item_key .. "_cost"] = 0
            else
                player[item_key .. "_cost"] = math.floor(player[item_key] * avg_cost_per_unit)
            end

            port[item_key] = port[item_key] + qty_to_trade
            player.trades = (player.trades or 0) + 1
        else
            local total_cost = qty_to_trade * price
            player.credits = player.credits - total_cost

            local current_qty = player[item_key] or 0
            local current_cost = player[item_key .. "_cost"] or 0

            player[item_key] = current_qty + qty_to_trade
            player[item_key .. "_cost"] = current_cost + total_cost
            port[item_key] = port[item_key] - qty_to_trade
            player.trades = (player.trades or 0) + 1
        end

        save_player(session, player)
        save_sectors(sectors)
        app.port_menu(session, player)
    end)
end

-- ---------------------------------------------------------------------------
-- CENTRAL STARDOCK (SHIPYARD, BANK, OUTFITTER)
-- ---------------------------------------------------------------------------

function app.stardock_menu(session, player)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print("==============================================================")
    term.move_to(2, 6)
    term.print("               ALPHA STARDOCK PRIME - CENTRAL HUB              ")
    term.move_to(2, 7)
    term.print("==============================================================")

    term.move_to(2, 9)
    term.set_color(14, 0)
    term.print(string.format("Commander %s   |   Credits: %d cr   |   Bank Vault: %d cr", player.nickname, player.credits, player.bank or 0))

    term.define_form(70)
    term.add_submit_button("shipyard", 2, 12)
    term.add_submit_button("outfitter", 16, 12)
    term.add_submit_button("bank", 30, 12)
    term.add_submit_button("launch", 42, 12)
    term.flush_form()

    session.await_input(70, function(sub)
        if type(sub) == "string" then app.stardock_menu(session, player) return end
        local act = sub.submit
        if act == "shipyard" then
            app.shipyard_menu(session, player)
        elseif act == "outfitter" then
            app.outfitter_menu(session, player)
        elseif act == "bank" then
            app.bank_menu(session, player)
        elseif act == "launch" then
            app.view_sector(session, player, "Launched into Sector " .. START_SECTOR_ID .. ".")
        else
            app.stardock_menu(session, player)
        end
    end)
end

function app.shipyard_menu(session, player)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print("=== STARDOCK SHIPYARD ===")
    term.move_to(2, 7)
    term.set_color(14, 0)
    term.print("Ship Class          Max Holds  Drones  Shields  Fuel Tank  Price")
    term.move_to(2, 8)
    term.print("----------          ---------  ------  -------  ---------  -----")

    for i, ship in ipairs(SHIP_CLASSES) do
        term.move_to(2, 8 + i)
        local is_curr = (player.ship_class or 1) == i
        term.set_color(is_curr and 10 or 15, 0)
        local marker = is_curr and "[OWNED]" or string.format("%d cr", ship.price)
        term.print(string.format("%-18s  %-9d  %-6d  %-7d  %-9d  %s", ship.name, ship.holds_max, ship.max_fighters, ship.max_shields, ship.fuel_max, marker))
    end

    term.define_form(75)
    term.add_submit_button("buy_hauler", 2, 15)
    term.add_submit_button("buy_freighter", 18, 15)
    term.add_submit_button("buy_dreadnought", 36, 15)
    term.add_submit_button("back", 56, 15)
    term.flush_form()

    session.await_input(75, function(sub)
        if type(sub) == "string" then app.shipyard_menu(session, player) return end
        local act = sub.submit
        if act == "back" then app.stardock_menu(session, player) return end

        local target_class = 1
        if act == "buy_hauler" then target_class = 2
        elseif act == "buy_freighter" then target_class = 3
        elseif act == "buy_dreadnought" then target_class = 4 end

        local target_ship = SHIP_CLASSES[target_class]
        if (player.ship_class or 1) >= target_class then
            app.stardock_menu(session, player)
            return
        end

        if player.credits >= target_ship.price then
            player.credits = player.credits - target_ship.price
            player.ship_class = target_class
            player.holds = target_ship.holds_base
            player.fuel = target_ship.fuel_max
            save_player(session, player)
            app.stardock_menu(session, player)
        else
            app.stardock_menu(session, player)
        end
    end)
end

function app.outfitter_menu(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1]
    local nav_info = NAV_COMPUTERS[player.nav_level or 1]
    local next_nav = NAV_COMPUTERS[(player.nav_level or 1) + 1]

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print("=== NAVAL OUTFITTER, ARMORY & NAV-DOCK ===")
    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(string.format("Cargo Holds: %d / %d max (Upgrade: 450 cr / +5 holds)", player.holds, ship_info.holds_max))
    term.move_to(2, 8)
    term.print(string.format("Combat Drones: %d / %d max (50 cr each) | Shields: %d / %d (75 cr each)", player.fighters, ship_info.max_fighters, player.shields, ship_info.max_shields))
    term.move_to(2, 9)
    term.print(string.format("Fuel Tank: %d / %d max (Refuel: 1 cr/unit)", player.fuel, ship_info.fuel_max))
    term.move_to(2, 10)
    term.set_color(14, 0)
    if next_nav then
        term.print(string.format("Nav Computer: %s -> %s (%d cr | %d Jumps, %d Favs)", nav_info.name, next_nav.name, next_nav.price, next_nav.max_jumps, next_nav.max_favorites))
    else
        term.print(string.format("Nav Computer: %s [MAX UPGRADE]", nav_info.name))
    end

    term.define_form(80)
    term.add_submit_button("buy_5holds", 2, 12)
    term.add_submit_button("buy_10fighters", 18, 12)
    term.add_submit_button("buy_10shields", 36, 12)
    term.add_submit_button("top_fuel", 54, 12)
    if next_nav then
        term.add_submit_button("upgrade_nav", 2, 14)
        term.add_submit_button("back", 18, 14)
    else
        term.add_submit_button("back", 2, 14)
    end
    term.flush_form()

    session.await_input(80, function(sub)
        if type(sub) == "string" then app.outfitter_menu(session, player) return end
        local act = sub.submit
        if act == "back" then app.stardock_menu(session, player) return end

        if act == "buy_5holds" then
            if player.holds + 5 <= ship_info.holds_max and player.credits >= 450 then
                player.credits = player.credits - 450
                player.holds = player.holds + 5
                save_player(session, player)
            end
        elseif act == "buy_10fighters" then
            local to_buy = math.min(10, ship_info.max_fighters - player.fighters)
            local cost = to_buy * 50
            if to_buy > 0 and player.credits >= cost then
                player.credits = player.credits - cost
                player.fighters = player.fighters + to_buy
                save_player(session, player)
            end
        elseif act == "buy_10shields" then
            local to_buy = math.min(10, ship_info.max_shields - player.shields)
            local cost = to_buy * 75
            if to_buy > 0 and player.credits >= cost then
                player.credits = player.credits - cost
                player.shields = player.shields + to_buy
                save_player(session, player)
            end
        elseif act == "top_fuel" then
            local missing = ship_info.fuel_max - player.fuel
            local max_afford = player.credits
            local to_add = math.min(missing, max_afford)
            if to_add > 0 then
                player.credits = player.credits - to_add
                player.fuel = player.fuel + to_add
                save_player(session, player)
            end
        elseif act == "upgrade_nav" and next_nav then
            if player.credits >= next_nav.price then
                player.credits = player.credits - next_nav.price
                player.nav_level = (player.nav_level or 1) + 1
                save_player(session, player)
            end
        end
        app.outfitter_menu(session, player)
    end)
end

function app.bank_menu(session, player)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print("=== GALACTIC COMMERCE BANK ===")
    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(string.format("Cash on Hand: %d cr    |    Vault Balance: %d cr", player.credits, player.bank or 0))

    term.define_form(85)
    term.add_submit_button("deposit_all", 2, 10)
    term.add_submit_button("deposit_half", 18, 10)
    term.add_submit_button("withdraw_1000", 36, 10)
    term.add_submit_button("withdraw_all", 54, 10)
    term.add_submit_button("back", 2, 12)
    term.flush_form()

    session.await_input(85, function(sub)
        if type(sub) == "string" then app.bank_menu(session, player) return end
        local act = sub.submit
        if act == "back" then app.stardock_menu(session, player) return end

        if act == "deposit_all" then
            player.bank = (player.bank or 0) + player.credits
            player.credits = 0
            save_player(session, player)
        elseif act == "deposit_half" then
            local half = math.floor(player.credits / 2)
            player.bank = (player.bank or 0) + half
            player.credits = player.credits - half
            save_player(session, player)
        elseif act == "withdraw_1000" then
            local take = math.min(1000, player.bank or 0)
            player.bank = (player.bank or 0) - take
            player.credits = player.credits + take
            save_player(session, player)
        elseif act == "withdraw_all" then
            player.credits = player.credits + (player.bank or 0)
            player.bank = 0
            save_player(session, player)
        end
        app.bank_menu(session, player)
    end)
end

-- ---------------------------------------------------------------------------
-- SENSORS, STATUS, & LEADERBOARD
-- ---------------------------------------------------------------------------

function app.scan_sector(session, player)
    local sectors = get_sectors()
    local sec = sectors[player.sector]
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print(string.format("=== LONG RANGE SCAN (Sector %d [%02d,%02d,%d]) ===", player.sector, cur_x, cur_y, cur_z))

    if sec.hazard == "COSMIC_STORM" then
        term.move_to(2, 7)
        term.set_color(12, 0)
        term.print(">>> SENSOR ARRAY SCRAMBLED BY IONIZED STORM INTERFERENCE <<<")
        term.move_to(2, 9)
        term.set_color(8, 0)
        term.print("Sensor telemetry offline. Clear nebula to restore LRS feed.")
    else
        term.move_to(2, 7)
        term.set_color(14, 0)
        term.print("Vector    Target   Coords       Port Status           Hazards")
        term.move_to(2, 8)
        term.print("------    ------   ------       -----------           -------")

        player.explored = player.explored or {}

        for idx, dest in ipairs(sec.warps or {}) do
            player.explored[dest] = true
            term.move_to(2, 8 + idx)
            local d_sec = sectors[dest]
            local dx, dy, dz = to_coords(dest)
            local dir_name, dir_code = get_direction_name(player.sector, dest)
            local port_str = "Deep Space (None)"
            if dest == START_SECTOR_ID then
                port_str = "Stardock Prime"
            elseif d_sec and d_sec.port then
                local fav_tag = is_favorite_sector(player, dest) and " [*]" or ""
                port_str = string.format("Class %d (%s)%s", d_sec.port.class, d_sec.port.name or "Port", fav_tag)
            end
            local hazard_str = (d_sec and d_sec.hazard) or "Clear"

            term.set_color(15, 0)
            term.print(string.format("[%-1s] %-5s  Sec %-4d [%02d,%02d,%d]   %-20s  %s", dir_code, dir_name, dest, dx, dy, dz, port_str, hazard_str))
        end
    end

    term.define_form(90)
    term.add_submit_button("back", 2, 16)
    term.flush_form()

    session.await_input(90, function() app.view_sector(session, player, "") end)
end

function app.view_status(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]
    local nav_info = NAV_COMPUTERS[player.nav_level or 1] or NAV_COMPUTERS[1]
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(14, 0)
    term.print("=== COMMANDER DOSSIER & MANIFEST ===")

    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(string.format("Commander: %-15s   Ship Class: %s", player.nickname, ship_info.name))
    term.move_to(2, 8)
    term.print(string.format("Location:  Sector %d [%02d,%02d,%d]   Turns: %d/%d   Fuel: %d/%d", player.sector, cur_x, cur_y, cur_z, player.turns, MAX_TURNS, player.fuel, ship_info.fuel_max))
    term.move_to(2, 9)
    term.print(string.format("Cash:      %-10d cr   Bank Vault: %d cr      Nav: %s", player.credits, player.bank or 0, nav_info.name))
    term.move_to(2, 10)
    term.print(string.format("Net Worth: %-10d cr   Pirates Vanquished: %d", calc_net_worth(player), player.kills or 0))

    term.move_to(2, 12)
    term.set_color(11, 0)
    term.print(string.format("Cargo Hold Manifest: %d / %d bays utilized", (player.ore + player.org + player.eqp), player.holds))

    local avg_ore = get_cargo_avg_cost(player, "ore")
    local avg_org = get_cargo_avg_cost(player, "org")
    local avg_eqp = get_cargo_avg_cost(player, "eqp")

    term.move_to(2, 13)
    term.print(string.format("  * Fuel Ore:  %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.ore, avg_ore, player.ore_cost or 0))
    term.move_to(2, 14)
    term.print(string.format("  * Organics:  %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.org, avg_org, player.org_cost or 0))
    term.move_to(2, 15)
    term.print(string.format("  * Equipment: %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.eqp, avg_eqp, player.eqp_cost or 0))

    term.define_form(95)
    term.add_submit_button("back", 2, 17)
    term.flush_form()

    session.await_input(95, function() app.view_sector(session, player, "") end)
end

function app.view_leaderboard(session, player)
    local board = db.get("vt_leaderboard", "scores") or {}
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(14, 0)
    term.print("=== GALACTIC HALL OF FAME ===")
    term.move_to(2, 7)
    term.set_color(11, 0)
    term.print("Rank   Commander           Vessel Class        Sector   Kills   Net Worth")
    term.move_to(2, 8)
    term.print("----   ---------           ------------        ------   -----   ---------")

    for i = 1, math.min(10, math.max(1, #board)) do
        term.move_to(2, 8 + i)
        if board[i] then
            local e = board[i]
            local is_me = e.node_id == session.node_id()
            term.set_color(is_me and 10 or 15, 0)
            term.print(string.format("#%-4d  %-18s  %-18s  %-7d  %-5d   %d cr", i, e.nickname or "Unknown", e.ship or "Scout", e.sector or START_SECTOR_ID, e.kills or 0, e.net_worth or 0))
        else
            term.set_color(8, 0)
            term.print(string.format("#%-4d  %-18s  %-18s  %-7s  %-5s   %s", i, "---", "---", "-", "-", "-"))
        end
    end

    term.define_form(99)
    term.add_submit_button("back", 2, 19)
    term.flush_form()

    session.await_input(99, function() app.view_sector(session, player, "") end)
end

return app
