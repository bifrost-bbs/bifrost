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

local MAX_TURNS = 100
local WARP_FUEL_COST = 3
local ORE_TO_FUEL_RATIO = 10

-- Ship Classes: name, holds_base, holds_max, max_fighters, max_shields, fuel_max, price
local SHIP_CLASSES = {
    [0] = { name = "Escape Pod", holds_base = 0, holds_max = 0, max_fighters = 0, max_shields = 0, fuel_max = 0, price = 0 },
    [1] = { name = "Scout Sloop", holds_base = 20, holds_max = 35, max_fighters = 25, max_shields = 25, fuel_max = 30, price = 300 },
    [2] = { name = "Merchant Hauler", holds_base = 50, holds_max = 80, max_fighters = 60, max_shields = 60, fuel_max = 60, price = 4500 },
    [3] = { name = "Armored Freighter", holds_base = 100, holds_max = 150, max_fighters = 120, max_shields = 120, fuel_max = 120, price = 14000 },
    [4] = { name = "Dreadnought Cruiser", holds_base = 200, holds_max = 300, max_fighters = 250, max_shields = 250, fuel_max = 250, price = 42000 }
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
    [1] = {1, 0, 0, code = "BSS", name = "BSS (Ore Buy / Org Sell / Eqp Sell)"},
    [2] = {0, 1, 0, code = "SBS", name = "SBS (Ore Sell / Org Buy / Eqp Sell)"},
    [3] = {0, 0, 1, code = "SSB", name = "SSB (Ore Sell / Org Sell / Eqp Buy)"},
    [4] = {0, 1, 1, code = "SBB", name = "SBB (Ore Sell / Org Buy / Eqp Buy)"},
    [5] = {1, 0, 1, code = "BSB", name = "BSB (Ore Buy / Org Sell / Eqp Buy)"},
    [6] = {1, 1, 0, code = "BBS", name = "BBS (Ore Buy / Org Buy / Eqp Sell)"},
    [7] = {1, 1, 1, code = "BBB", name = "BBB (Import Station - Buys All)"},
    [8] = {0, 0, 0, code = "SSS", name = "SSS (Industrial Hub - Sells All)"}
}

local BASE_PRICES = { ore = 15, org = 35, eqp = 80 }

local CANTINA_VERBS = {
    "Drifting", "Spinning", "Blazing", "Frozen", "Warping",
    "Orbiting", "Quantum", "Roaming", "Sleeping", "Fading",
    "Shining", "Shattered", "Gilded", "Smoking", "Howling",
    "Stray", "Silent", "Lost", "Twisted", "Wandering"
}

local CANTINA_NOUNS = {
    "Pulsar", "Comet", "Nebula", "Quasar", "Supernova",
    "Asteroid", "Cosmonaut", "Vagabond", "Corsair", "Star",
    "Voidfarer", "Moon", "Eclipse", "Horizon", "Singularity",
    "Meteor", "Beacon", "Black Hole", "Constellation", "Spaceway"
}

local function get_cantina_name(sector_id, port)
    if sector_id == START_SECTOR_ID then
        return "The Singing Pulsar"
    end
    if port and port.cantina_name then
        return port.cantina_name
    end
    local v_idx = ((sector_id * 17 + 5) % #CANTINA_VERBS) + 1
    local n_idx = ((sector_id * 31 + 11) % #CANTINA_NOUNS) + 1
    return string.format("The %s %s", CANTINA_VERBS[v_idx], CANTINA_NOUNS[n_idx])
end

local function get_port_services(sector_id, port)
    if sector_id == START_SECTOR_ID then
        return "H", true, true, true
    end
    if port and port.service_tier then
        return port.service_tier, (port.has_bank or false), (port.has_outfitter or false), (port.has_shipyard or false)
    end
    local roll = (sector_id * 53 + 7) % 100
    if roll < 5 then
        return "H", true, true, true
    elseif roll < 15 then
        return "B", true, true, false
    else
        return "-", false, false, false
    end
end

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

    for i = 1, NUM_SECTORS do
        local port = nil
        if i == START_SECTOR_ID then
            port = {
                class = 0,
                name = "Alpha Stardock Prime",
                ore = 9999,
                org = 9999,
                eqp = 9999,
                drink_price = 50,
                has_cantina = true,
                cantina_name = "The Singing Pulsar",
                service_tier = "H",
                has_bank = true,
                has_outfitter = true,
                has_shipyard = true
            }
        elseif math.random(1, 100) <= 60 then
            local p_class = math.random(1, 8)
            local has_c = (math.random(1, 100) <= 5)
            local c_name = nil
            if has_c then
                local v_idx = math.random(1, #CANTINA_VERBS)
                local n_idx = math.random(1, #CANTINA_NOUNS)
                c_name = string.format("The %s %s", CANTINA_VERBS[v_idx], CANTINA_NOUNS[n_idx])
            end

            local s_roll = math.random(1, 100)
            local s_tier = "-"
            local h_bank = false
            local h_outfit = false
            local h_ship = false
            if s_roll <= 5 then
                s_tier = "H"
                h_bank = true
                h_outfit = true
                h_ship = true
            elseif s_roll <= 15 then
                s_tier = "B"
                h_bank = true
                h_outfit = true
            end

            port = {
                class = p_class,
                name = "Port " .. string.char(64 + p_class) .. "-" .. i,
                ore = math.random(600, 3000),
                org = math.random(400, 2500),
                eqp = math.random(200, 1500),
                drink_price = math.random(25, 200),
                has_cantina = has_c,
                cantina_name = c_name,
                service_tier = s_tier,
                has_bank = h_bank,
                has_outfitter = h_outfit,
                has_shipyard = h_ship
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

local _cached_sectors = nil

local function get_sectors()
    if _cached_sectors and #_cached_sectors >= NUM_SECTORS then
        return _cached_sectors
    end
    local s = db.get("vt_sectors", "all")
    if not s or type(s) ~= "table" or #s < NUM_SECTORS then
        s = init_universe()
    end
    _cached_sectors = s
    return s
end

local function save_sector(sector_id, sector)
    if _cached_sectors then
        _cached_sectors[sector_id] = sector
    end
    db.set("vt_sectors", sector_id, sector)
end

local function save_sectors(sectors)
    _cached_sectors = sectors
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
        insurance_level = 0,
        turns = MAX_TURNS,
        ship_class = 1,
        nav_level = 1,
        fuel = 30,
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
    local today = (session.date_str and session.date_str()) or "day-0"
    if not p or type(p) ~= "table" then
        p = init_player(session)
        p.last_turn_date = today
        db.set("vt_players", session.node_id(), p)
    end

    local user = db.get("users", session.node_id()) or {}
    if user.nickname then p.nickname = user.nickname end

    if p.last_turn_date ~= today then
        p.turns = MAX_TURNS
        p.last_turn_date = today
        db.set("vt_players", session.node_id(), p)
    elseif not p.turns then
        p.turns = MAX_TURNS
        p.last_turn_date = today
        db.set("vt_players", session.node_id(), p)
    end

    local ship_info = SHIP_CLASSES[p.ship_class or 1] or SHIP_CLASSES[1]
    if p.fuel == nil then p.fuel = ship_info.fuel_max end
    if p.nav_level == nil then p.nav_level = 1 end
    if p.insurance_level == nil then p.insurance_level = 0 end
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
-- MAIN NAVIGATION & SECTOR VIEW (USING TEMPLATE & MENU ASSETS)
-- ---------------------------------------------------------------------------

function app.on_start(session)
    local player = get_player(session)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    term.clear()
    term.render_asset("voidtrader_banner")

    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print("=== INTERSTELLAR PILOT DISPATCH & TERMINAL ===")

    term.move_to(2, 7)
    term.set_color(15, 0)
    local net_worth = calc_net_worth(player)
    term.print(string.format("Pilot: %-12s  Ship: %-14s  Worth: %d cr  Turns: %d/%d", player.nickname, ship_info.name, net_worth, player.turns or 100, MAX_TURNS))

    term.move_to(2, 9)
    term.set_color(11, 0)
    term.print("Welcome to the outer rim, Commander. The Void Trader expanse spans 2,250")
    term.move_to(2, 10)
    term.print("charted sectors with volatile trading hubs, cosmic hazards, and ruthless")
    term.move_to(2, 11)
    term.print("pirate corsairs. Establish trade routes, upgrade your hull and combat")
    term.move_to(2, 12)
    term.print("drones, insure your assets, and compete for galactic fortune and glory!")

    term.render_menu("entry_menu", {
        resume = true,
        new_game = true,
        leaderboard = true,
        help = true,
        exit = true
    })
    term.flush_form()

    session.await_input(10, function(sub)
        if type(sub) == "string" then app.on_start(session) return end
        local act = sub.submit

        if act == "resume" then
            app.view_sector(session, player, "Command bridge online. All systems nominal.")
        elseif act == "new_game" then
            save_player(session, player)
            local new_p = init_player(session)
            new_p.last_turn_date = (session.date_str and session.date_str()) or "day-0"
            db.set("vt_players", session.node_id(), new_p)
            app.view_sector(session, new_p, "New voyage initiated at Stardock Alpha Prime.")
        elseif act == "leaderboard" then
            app.view_leaderboard(session, player, true)
        elseif act == "help" then
            app.view_help(session, 1)
        elseif act == "exit" then
            save_player(session, player)
            session.exec_app("main_menu")
        else
            app.on_start(session)
        end
    end)
end

function app.view_help(session, page_num)
    page_num = math.max(1, math.min(6, page_num or 1))
    local help_assets = {
        "help_basics",
        "help_trading",
        "help_navigation",
        "help_combat",
        "help_insurance",
        "help_cantinas"
    }

    term.clear()
    term.move_to(2, 1)
    term.set_color(14, 0)
    term.print(string.format("=== GALACTIC ARCHIVES: PILOT GUIDE (PAGE %d OF 6) ===", page_num))

    term.move_to(2, 3)
    term.render_asset(help_assets[page_num])

    term.render_menu("help_menu", {
        next = (page_num < 6),
        prev = (page_num > 1),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(15, function(sub)
        if type(sub) == "string" then app.view_help(session, page_num) return end
        local act = sub.submit

        if act == "next" and page_num < 6 then
            app.view_help(session, page_num + 1)
        elseif act == "prev" and page_num > 1 then
            app.view_help(session, page_num - 1)
        elseif act == "return" then
            app.on_start(session)
        else
            app.view_help(session, page_num)
        end
    end)
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
    term.move_to(2, 1)

    -- Render HUD using cached Template Asset (only ~20 bytes over the air!)
    term.render_template("sector_hud", {
        tostring(player.sector),
        string.format("%02d", cur_x),
        string.format("%02d", cur_y),
        tostring(cur_z),
        tostring(player.turns),
        tostring(player.credits),
        tostring(player.bank or 0),
        ship_info.name,
        tostring(player.fuel),
        tostring(ship_info.fuel_max),
        tostring(player.ore + player.org + player.eqp),
        tostring(player.holds),
        tostring(player.fighters),
        tostring(player.shields)
    })

    -- Directional Warp Links String
    term.move_to(2, 3)
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

    term.move_to(2, 4)
    local fav_marker = is_fav and " [* FAV]" or ""
    if player.sector == START_SECTOR_ID then
        term.set_color(10, 0)
        term.print("Facilities: [ Stardock Prime ] (Shipyard, Bank, Outfitter, The Singing Pulsar)" .. fav_marker)
    elseif sec.port then
        term.set_color(10, 0)
        local p_info = PORT_CLASSES[sec.port.class] or { code = "???", name = "Trading Station" }
        local s_tier, has_b, has_o, has_s = get_port_services(player.sector, sec.port)
        local p_code = string.format("%s%s", p_info.code or "???", s_tier)
        local has_cantina = sec.port.has_cantina or ((player.sector * 37 + 13) % 100 < 5)
        local cantina_tag = has_cantina and string.format(" | Cantina: %s", get_cantina_name(player.sector, sec.port)) or ""
        term.print(string.format("Port: Class %d [%s] (%s)%s%s", sec.port.class, p_code, sec.port.name or ("Port " .. player.sector), cantina_tag, fav_marker))
    else
        term.set_color(8, 0)
        term.print("Port: None in this sector (Deep Space)")
    end

    if sec.hazard == "BLACK_HOLE" then
        term.move_to(2, 5)
        term.set_color(12, 0)
        term.print("HAZARD: BLACK HOLE SINGULARITY (Extreme Gravity Well!)")
    elseif sec.hazard == "WORMHOLE" then
        term.move_to(2, 5)
        term.set_color(13, 0)
        term.print("ANOMALY: UNSTABLE WORMHOLE RIFT (Trans-Galactic Conduit)")
    elseif sec.hazard == "ASTEROID_FIELD" then
        term.move_to(2, 5)
        term.set_color(14, 0)
        term.print("ANOMALY: DENSE ASTEROID BELT (Fuel Ore Deposits Available)")
    elseif sec.hazard == "COSMIC_STORM" then
        term.move_to(2, 5)
        term.set_color(11, 0)
        term.print("HAZARD: IONIZED COSMIC STORM (Sensor Static & Shield Drain)")
    elseif sec.hazard == "DERELICT_GRAVEYARD" then
        term.move_to(2, 5)
        term.set_color(15, 0)
        term.print("ANOMALY: DERELICT SHIP GRAVEYARD (Salvage Potential)")
    end

    local next_course_hop = nil
    if player.plotted_course and #player.plotted_course > 0 then
        next_course_hop = player.plotted_course[1]
        term.move_to(2, 6)
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
        term.move_to(2, 6)
        term.set_color(12, 0)
        if sec.port then
            term.print("NOTICE: Out of warp fuel! Dock at the starport to refuel your tanks.")
        else
            term.print("STRANDED: Out of fuel in deep space! Send distress beacon.")
        end
    end

    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(msg or "")

    -- Render Interactive Menu via Cached Menu Asset (only 7 bytes over the air!)
    term.render_menu("sector_menu", {
        north = (dir_map["north"] ~= nil),
        south = (dir_map["south"] ~= nil),
        east = (dir_map["east"] ~= nil),
        west = (dir_map["west"] ~= nil),
        up = (dir_map["up"] ~= nil),
        down = (dir_map["down"] ~= nil),
        autowarp = (next_course_hop ~= nil),
        chart = true,
        stardock = (player.sector == START_SECTOR_ID),
        dock = (sec.port ~= nil and player.sector ~= START_SECTOR_ID),
        fav = (sec.port ~= nil and not is_fav),
        unfav = (sec.port ~= nil and is_fav),
        wormhole = (sec.hazard == "WORMHOLE"),
        mine_ore = (sec.hazard == "ASTEROID_FIELD"),
        salvage = (sec.hazard == "DERELICT_GRAVEYARD"),
        slingshot = (sec.hazard == "BLACK_HOLE"),
        refuel = (player.ore > 0 and player.fuel < ship_info.fuel_max),
        plot = true,
        scan = true,
        status = true,
        ranks = true,
        exit = true,
        distress = (is_stranded and sec.port == nil)
    })
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
            app.view_leaderboard(session, player, false)
        elseif act == "exit" then
            save_player(session, player)
            session.exec_app("main_menu")
        else
            app.view_sector(session, player, "")
        end
    end)
end

-- ---------------------------------------------------------------------------
-- 3D STARCHART VIEWER
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
            local col = 8

            if is_here then
                ch = "@"
                col = 14
            elseif is_exp then
                local s_data = sectors[sec_id]
                if sec_id == START_SECTOR_ID then
                    ch = "S"
                    col = 10
                elseif s_data and s_data.hazard == "BLACK_HOLE" then
                    ch = "B"
                    col = 12
                elseif s_data and s_data.hazard == "WORMHOLE" then
                    ch = "W"
                    col = 13
                elseif s_data and s_data.hazard == "ASTEROID_FIELD" then
                    ch = "A"
                    col = 14
                elseif s_data and s_data.hazard == "COSMIC_STORM" then
                    ch = "C"
                    col = 11
                elseif s_data and s_data.hazard == "DERELICT_GRAVEYARD" then
                    ch = "D"
                    col = 15
                elseif s_data and s_data.port then
                    ch = "P"
                    col = 10
                else
                    ch = "+"
                    col = 7
                end
            end

            term.set_color(col, 0)
            term.print(ch)
        end

        term.set_color(7, 0)
        term.print("|")
    end

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
        app.player_destroyed(session, player, "Vessel pulled past event horizon and crushed by singularity.")
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
    term.move_to(2, 1)
    term.set_color(11, 0)
    term.print(string.format("=== HYPERSPACE COURSE PLOTTER (%s) ===", nav_info.name))
    term.move_to(2, 2)
    term.set_color(14, 0)
    term.print(string.format("Position: Sec %d [%02d,%02d,%d] | Max Jumps Plottable: %d", player.sector, cur_x, cur_y, cur_z, nav_info.max_jumps))

    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print("Favorite Starports:")
    if player.favorites and #player.favorites > 0 then
        for idx, fav_sec in ipairs(player.favorites) do
            if idx <= 4 then
                local sec = sectors[fav_sec]
                local fx, fy, fz = to_coords(fav_sec)
                local port_name = (sec and sec.port and sec.port.name) or "Deep Space"
                term.move_to(4, 3 + idx)
                term.set_color(10, 0)
                term.print(string.format("[%d] Sector %-4d [%02d,%02d,%d] - %s", idx, fav_sec, fx, fy, fz, port_name))
            end
        end
    else
        term.move_to(4, 4)
        term.set_color(8, 0)
        term.print("No favorite ports saved. (Press [fav] at any starport)")
    end

    term.define_form(25)
    term.move_to(2, 8)
    term.set_color(15, 0)
    term.print("Enter Target Sector (1 - " .. NUM_SECTORS .. "): ")
    term.add_input_field("target", 34, 8, 5, "")

    term.add_submit_button("plot_course", 2, 10)
    term.add_submit_button("clear_course", 16, 10)
    term.add_submit_button("back", 32, 10)
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

    if (player.ship_class or 1) == 0 then
        app.view_sector(session, player, "Escape Pod has no warp engines! Dock at Stardock to acquire a ship.")
        return
    end

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

    local prev_sector = player.sector
    player.sector = dest
    player.turns = player.turns - 1
    player.fuel = player.fuel - WARP_FUEL_COST

    if player.merc_contract and player.merc_contract > 0 then
        player.merc_contract = player.merc_contract - 1
        if player.merc_contract <= 0 then
            player.merc_contract = nil
        end
    end

    save_player(session, player)

    local dest_sec = sectors[dest]

    if dest_sec.hazard == "COSMIC_STORM" and player.shields > 0 then
        local dmg = math.random(1, 3)
        player.shields = math.max(0, player.shields - dmg)
        save_player(session, player)
    end

    local roll = math.random(1, 100)
    if dest ~= START_SECTOR_ID and roll <= 16 then
        app.pirate_encounter(session, player, math.random(8, 25))
    elseif dest ~= START_SECTOR_ID and roll <= 26 then
        app.derelict_salvage(session, player)
    else
        local _, code, _ = get_direction_name(prev_sector, dest)
        app.view_sector(session, player, string.format("Warped into Sector %d [%s vector].", dest, code))
    end
end

-- ---------------------------------------------------------------------------
-- COMBAT & SPACE ENCOUNTERS
-- ---------------------------------------------------------------------------

function app.pirate_encounter(session, player, enemy_fighters, combat_msg)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    term.clear()
    term.move_to(2, 1)
    term.set_color(12, 0)
    term.print("RED ALERT: Void Marauder Corsair dropping out of warp!")

    term.move_to(2, 3)
    term.set_color(7, 0)
    term.print(string.format("Enemy Combat Drones: %-3d", enemy_fighters))
    term.move_to(2, 4)
    term.print(string.format("Your Fleet: %d Fighters | %d / %d Deflector Shields | %d Fuel", player.fighters, player.shields, ship_info.max_shields, player.fuel))

    term.move_to(2, 5)
    if player.merc_contract and player.merc_contract > 0 then
        term.set_color(10, 0)
        term.print(string.format("Mercenary Escort: Vanguard Wing Online (+10 Firepower | %d Jumps Left)", player.merc_contract))
    else
        term.set_color(8, 0)
        term.print("Mercenary Escort: None (Solo Flight)")
    end

    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print(combat_msg or "Enemy weapons systems locked onto your ship hull!")

    term.render_menu("combat_menu", {
        attack = (player.fighters > 0 or (player.merc_contract and player.merc_contract > 0)),
        shields_up = true,
        flee = true
    })
    term.flush_form()

    session.await_input(30, function(sub)
        if type(sub) == "string" then app.pirate_encounter(session, player, enemy_fighters) return end
        local act = sub.submit

        if act == "attack" then
            local merc_bonus = (player.merc_contract and player.merc_contract > 0) and math.random(5, 12) or 0
            local p_dmg = math.random(2, math.max(3, math.floor(player.fighters * 0.8))) + merc_bonus
            local e_dmg = math.random(2, math.max(3, math.floor(enemy_fighters * 0.7)))

            if merc_bonus > 0 and e_dmg > 0 then
                -- Mercenary absorbs up to 4 enemy damage
                local merc_absorb = math.min(4, e_dmg)
                e_dmg = e_dmg - merc_absorb
            end

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
                app.player_destroyed(session, player, "Your vessel was torn apart by Void Marauders.")
            else
                local msg = string.format("Fighters engaged! Inflicted %d damage, took %d enemy damage.", p_dmg, e_dmg)
                app.pirate_encounter(session, player, enemy_fighters, msg)
            end
        elseif act == "shields_up" then
            -- Tactical shield overcharge & deflection
            local shield_boost = 0
            if player.fuel >= 1 then
                player.fuel = player.fuel - 1
                shield_boost = 3
                player.shields = math.min(ship_info.max_shields, player.shields + shield_boost)
            end

            -- Enemy damage heavily deflected by 75%
            local raw_e_dmg = math.random(1, math.max(2, math.floor(enemy_fighters * 0.35)))
            local e_dmg = math.max(1, math.floor(raw_e_dmg * 0.25))

            local abs = math.min(player.shields, e_dmg)
            player.shields = player.shields - abs
            e_dmg = e_dmg - abs

            if player.shields <= 0 and player.fighters <= 0 then
                app.player_destroyed(session, player, "Deflector shields overloaded and vessel destroyed.")
            else
                local msg = string.format("[SHIELDS UP] Overcharged shields (+%d)! Deflected 75%% incoming fire (-%d absorbed).", shield_boost, abs)
                app.pirate_encounter(session, player, enemy_fighters, msg)
            end
        elseif act == "flee" then
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

                if player.fighters <= 0 and player.shields <= 0 then
                    app.player_destroyed(session, player, "Destroyed while attempting hyperdrive escape.")
                else
                    local msg = "Hyperdrive jump failed! Hull sustained laser fire while charging coils."
                    app.pirate_encounter(session, player, enemy_fighters, msg)
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

function app.player_destroyed(session, player, cause)
    local ins_lvl = player.insurance_level or 0
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    term.clear()
    term.move_to(2, 1)
    term.set_color(12, 0)
    term.print("==============================================================")
    term.move_to(2, 2)
    term.print("                 VESSEL CRITICAL CASUALTY                     ")
    term.move_to(2, 3)
    term.print("==============================================================")

    term.move_to(2, 4)
    term.set_color(15, 0)
    term.print("Cause of Destruction: " .. cause)

    term.move_to(2, 6)
    term.set_color(14, 0)
    if ins_lvl == 0 then
        term.print("INSURANCE COVERAGE: NONE (UNINSURED)")
        term.move_to(2, 7)
        term.set_color(12, 0)
        term.print(string.format("Bank Vault (%d cr) preserved. Cash, ship & cargo vaporized in deep space.", player.bank or 0))
    elseif ins_lvl == 1 then
        term.print("INSURANCE COVERAGE: BRONZE POLICY")
        term.move_to(2, 7)
        term.set_color(10, 0)
        term.print(string.format("Bank Vault (%d cr) + 100 cr stipend & emergency Scout Sloop allocated.", player.bank or 0))
    elseif ins_lvl == 2 then
        term.print("INSURANCE COVERAGE: SILVER POLICY")
        term.move_to(2, 7)
        term.set_color(10, 0)
        local cash_retained = math.floor(player.credits * 0.5)
        term.print(string.format("Bank Vault (%d cr) + 50%% pocket cash (%d cr) preserved. %s hull allocated!", player.bank or 0, cash_retained, ship_info.name))
    elseif ins_lvl == 3 then
        term.print("INSURANCE COVERAGE: GOLD COMPREHENSIVE POLICY")
        term.move_to(2, 7)
        term.set_color(10, 0)
        term.print(string.format("Bank Vault (%d cr) + 100%% cash (%d cr) + %s hull, all cargo and upgrades fully restored!", player.bank or 0, player.credits, ship_info.name))
    end

    term.render_menu("death_menu", {
        rescue = true,
        main_menu = true
    })
    term.flush_form()

    session.await_input(40, function(sub)
        if type(sub) == "string" then app.player_destroyed(session, player, cause) return end
        local act = sub.submit

        if act == "rescue" then
            local p = init_player(session)
            if ins_lvl == 0 then
                p.bank = player.bank or 0
                p.credits = 0
                p.ship_class = 0
                p.holds = 0
                p.fuel = 0
                p.fighters = 0
                p.shields = 0
                p.ore = 0
                p.org = 0
                p.eqp = 0
                p.insurance_level = 0
            elseif ins_lvl == 1 then
                p.bank = player.bank or 0
                p.credits = 100
                p.ship_class = 1
                p.insurance_level = 0
            elseif ins_lvl == 2 then
                p.bank = player.bank or 0
                p.credits = math.floor(player.credits * 0.5) + 100
                p.ship_class = player.ship_class or 1
                p.nav_level = player.nav_level or 1
                local s_info = SHIP_CLASSES[p.ship_class] or SHIP_CLASSES[1]
                p.holds = s_info.holds_base
                p.fuel = s_info.fuel_max
                p.fighters = 15
                p.shields = 15
                p.insurance_level = 0
            elseif ins_lvl == 3 then
                p = player
                p.sector = START_SECTOR_ID
                p.turns = MAX_TURNS
                local s_info = SHIP_CLASSES[p.ship_class or 1] or SHIP_CLASSES[1]
                p.fuel = s_info.fuel_max
                p.insurance_level = 0 -- Gold policy claimed
            end
            p.sector = START_SECTOR_ID
            p.turns = MAX_TURNS
            save_player(session, p)
            if ins_lvl == 0 then
                app.view_sector(session, p, "Rescue tug towed your Escape Pod to Alpha Stardock. Visit Shipyard/Bank!")
            else
                app.view_sector(session, p, "Insurance underwriters towed replacement ship to Alpha Stardock Prime.")
            end
        else
            app.on_start(session)
        end
    end)
end

-- ---------------------------------------------------------------------------
-- COMMODITY TRADING AT STARPORTS (USING TABLE API & MENU ASSET)
-- ---------------------------------------------------------------------------

function app.port_menu(session, player, msg)
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
    term.move_to(2, 1)
    term.set_color(10, 0)
    local s_tier, has_bank, has_outfitter, has_shipyard = get_port_services(player.sector, port)
    local p_code = string.format("%s%s", p_rules.code or "???", s_tier)
    term.print(string.format("=== %s (Class %d [%s]) ===", port.name or "Commerce Post", port.class, p_code))

    local function build_row(key, name, port_amt, pl_amt, rule, base_p)
        local is_station_buying = (rule == 1)
        local price = is_station_buying and math.floor(base_p * 0.9) or math.floor(base_p * 1.15)
        local act_str = is_station_buying and string.format("BUY @ %d cr", price) or string.format("SELL @ %d cr", price)

        local cargo_str = (pl_amt > 0) and string.format("%d (%d cr)", pl_amt, get_cargo_avg_cost(player, key)) or "0 (-)"
        local margin_str = "---"
        if is_station_buying and pl_amt > 0 then
            local avg_p = get_cargo_avg_cost(player, key)
            local diff = price - avg_p
            local pct = avg_p > 0 and math.floor((diff / avg_p) * 100) or 0
            margin_str = string.format("%+d cr (%+d%%)", diff, pct)
        end

        return { name, tostring(port_amt), cargo_str, act_str, margin_str }, price
    end

    local row1, pr_ore = build_row("ore", "Fuel Ore", port.ore, player.ore, p_rules[1], BASE_PRICES.ore)
    local row2, pr_org = build_row("org", "Organics", port.org, player.org, p_rules[2], BASE_PRICES.org)
    local row3, pr_eqp = build_row("eqp", "Equipment", port.eqp, player.eqp, p_rules[3], BASE_PRICES.eqp)

    -- Render Table with clean structured formatting
    term.render_table(2, 3, {
        headers = { "Commodity", "Port Stock", "Hold (Avg)", "Action / Price", "Margin / Diff" },
        widths = { 11, 11, 14, 15, 16 },
        rows = { row1, row2, row3 },
        header_fg = 14,
        row_fg = 15,
        divider = true
    })

    term.move_to(2, 7)
    term.set_color(15, 0)
    local holds_used = player.ore + player.org + player.eqp
    term.print(string.format("Cash: %-8d cr   Holds: %d / %d   Fuel: %d / %d (Refuel: 1 cr/unit)", player.credits, (player.holds - holds_used), player.holds, player.fuel, ship_info.fuel_max))

    if msg and msg ~= "" then
        term.move_to(2, 8)
        term.set_color(12, 0)
        term.print(">>> " .. msg)
    end

    local has_cantina = port.has_cantina or ((player.sector * 37 + 13) % 100 < 5)
    term.render_menu("port_menu", {
        ore = true,
        org = true,
        eqp = true,
        refuel = (player.fuel < ship_info.fuel_max),
        outfitter = has_outfitter,
        bank = has_bank,
        shipyard = has_shipyard,
        cantina = has_cantina,
        ["return"] = true
    })
    term.flush_form()

    session.await_input(50, function(sub)
        if type(sub) == "string" then app.port_menu(session, player) return end
        local act = sub.submit

        if act == "return" or act == "depart" then
            app.view_sector(session, player, "Departed starport.")
            return
        end

        if act == "tavern" or act == "bar" then
            app.tavern_menu(session, player)
            return
        end

        if act == "outfitter" then
            app.outfitter_menu(session, player)
            return
        end

        if act == "bank" then
            app.bank_menu(session, player)
            return
        end

        if act == "shipyard" then
            app.shipyard_menu(session, player)
            return
        end

        if act == "refuel" or act == "refuel_tank" then
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
    term.move_to(2, 1)
    term.set_color(10, 0)
    term.print("=== COMMODITY TRANSACTION: " .. string.upper(item_name) .. " ===")

    term.move_to(2, 3)
    term.set_color(15, 0)
    if is_station_buying then
        term.print(string.format("Mode: Station is BUYING from you (You SELL) @ %d cr/unit", price))
        term.move_to(2, 4)
        term.print(string.format("In Cargo: %d units (Avg Paid: %d cr/unit) | Max Tradable: %d units", pl_amt, avg_paid, max_qty))

        term.move_to(2, 5)
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
        term.move_to(2, 4)
        term.print(string.format("Port Stock: %d | Empty Holds: %d | Max Afford: %d", port_amt, free_holds, math.floor(player.credits / price)))
        term.move_to(2, 5)
        if pl_amt > 0 then
            term.print(string.format("Current Cargo: %d units (Avg Paid: %d cr/unit) | Max Tradable: %d", pl_amt, avg_paid, max_qty))
        else
            term.print(string.format("Maximum Tradable: %d units", max_qty))
        end
    end

    term.move_to(2, 7)
    term.set_color(14, 0)
    term.print(string.format("Cash on Hand: %d cr   |   Holds: %d / %d", player.credits, (player.holds - free_holds), player.holds))

    term.define_form(55)
    term.move_to(2, 9)
    term.set_color(15, 0)
    term.print("Trade Quantity: ")
    term.add_input_field("qty", 18, 9, 6, tostring(max_qty))

    term.add_submit_button("trade", 2, 11)
    term.add_submit_button("trade_all", 14, 11)
    term.add_submit_button("cancel", 28, 11)
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
        save_sector(player.sector, sectors[player.sector])
        app.port_menu(session, player)
    end)
end

-- ---------------------------------------------------------------------------
-- CENTRAL STARDOCK (SHIPYARD, BANK, OUTFITTER)
-- ---------------------------------------------------------------------------

-- ---------------------------------------------------------------------------
-- CENTRAL STARDOCK (SHIPYARD, BANK, OUTFITTER, INSURANCE)
-- ---------------------------------------------------------------------------

function app.stardock_menu(session, player, banner_msg)
    term.clear()
    term.move_to(2, 1)
    term.set_color(10, 0)
    term.print("=== ALPHA STARDOCK PRIME - CENTRAL FEDERATION HUB ===")

    term.move_to(2, 3)
    term.set_color(14, 0)
    term.print(string.format("Commander %s   |   Credits: %d cr   |   Bank Vault: %d cr", player.nickname, player.credits, player.bank or 0))

    term.move_to(2, 4)
    if banner_msg and banner_msg ~= "" then
        term.set_color(12, 0)
        term.print(">>> " .. banner_msg)
    elseif (player.insurance_level or 0) == 0 then
        term.set_color(12, 0)
        term.print("STATUS: UNINSURED! Visit Insurance Underwriters [I] to protect your voyage.")
    else
        term.set_color(10, 0)
        local cov_names = { [1] = "Bronze Policy Active", [2] = "Silver Policy Active", [3] = "Gold Policy Active" }
        term.print(string.format("STATUS: %s (Single-use policy: re-purchase on loss)", cov_names[player.insurance_level] or "Insured"))
    end

    term.render_menu("stardock_menu", {
        shipyard = true,
        outfitter = true,
        bank = true,
        insurance = true,
        bar = true,
        ["return"] = true
    })
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
        elseif act == "insurance" then
            app.insurance_office(session, player)
        elseif act == "bar" then
            app.tavern_menu(session, player)
        elseif act == "return" then
            app.view_sector(session, player, "Returned to ship bridge in Sector " .. START_SECTOR_ID .. ".")
        else
            app.stardock_menu(session, player)
        end
    end)
end

function app.shipyard_menu(session, player)
    term.clear()
    term.move_to(2, 1)
    term.set_color(11, 0)
    term.print("=== STARDOCK SHIPYARD ===")

    local rows = {}
    for i, ship in ipairs(SHIP_CLASSES) do
        local is_curr = (player.ship_class or 1) == i
        local marker = is_curr and "[OWNED]" or string.format("%d cr", ship.price)
        table.insert(rows, {
            ship.name,
            tostring(ship.holds_max),
            tostring(ship.max_fighters),
            tostring(ship.max_shields),
            tostring(ship.fuel_max),
            marker
        })
    end

    term.render_table(2, 3, {
        headers = { "Ship Class", "Max Holds", "Drones", "Shields", "Fuel Tank", "Price" },
        widths = { 18, 9, 6, 7, 9, 10 },
        rows = rows,
        header_fg = 14,
        row_fg = 15,
        divider = true
    })

    local is_pod = (player.ship_class or 1) == 0
    term.render_menu("shipyard_menu", {
        sloop = is_pod,
        hauler = (player.credits >= SHIP_CLASSES[2].price and (player.ship_class or 1) < 2),
        freighter = (player.credits >= SHIP_CLASSES[3].price and (player.ship_class or 1) < 3),
        cruiser = (player.credits >= SHIP_CLASSES[4].price and (player.ship_class or 1) < 4),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(75, function(sub)
        if type(sub) == "string" then app.shipyard_menu(session, player) return end
        local function return_dock()
            if player.sector == START_SECTOR_ID then
                app.stardock_menu(session, player)
            else
                app.port_menu(session, player)
            end
        end

        if act == "return" then return_dock() return end

        local target_class = 1
        if act == "buy_sloop" then target_class = 1
        elseif act == "buy_hauler" then target_class = 2
        elseif act == "buy_freighter" then target_class = 3
        elseif act == "buy_cruiser" then target_class = 4 end

        local target_ship = SHIP_CLASSES[target_class]
        if (player.ship_class or 1) >= target_class and (player.ship_class or 1) > 0 then
            return_dock()
            return
        end

        if player.credits >= target_ship.price then
            player.credits = player.credits - target_ship.price
            player.ship_class = target_class
            player.holds = target_ship.holds_base
            player.fuel = target_ship.fuel_max
            player.fighters = 10
            player.shields = 10
            save_player(session, player)
            return_dock()
        elseif (player.ship_class or 1) == 0 and (player.bank or 0) == 0 and player.credits < target_ship.price then
            -- Bankruptcy relief for escape pod pilots with 0 in bank and 0 in pocket
            player.credits = 100
            player.ship_class = 1
            player.holds = target_ship.holds_base
            player.fuel = target_ship.fuel_max
            player.fighters = 10
            player.shields = 10
            save_player(session, player)
            return_dock()
        else
            return_dock()
        end
    end)
end

function app.outfitter_menu(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1]
    local nav_info = NAV_COMPUTERS[player.nav_level or 1]
    local next_nav = NAV_COMPUTERS[(player.nav_level or 1) + 1]

    term.clear()
    term.move_to(2, 1)
    term.set_color(11, 0)
    term.print("=== NAVAL OUTFITTER, ARMORY & NAV-DOCK ===")
    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print(string.format("Cargo Holds: %d / %d max (Upgrade: 450 cr / +5 holds)", player.holds, ship_info.holds_max))
    term.move_to(2, 4)
    term.print(string.format("Combat Drones: %d / %d max (50 cr each) | Shields: %d / %d (75 cr each)", player.fighters, ship_info.max_fighters, player.shields, ship_info.max_shields))
    term.move_to(2, 5)
    term.print(string.format("Fuel Tank: %d / %d max (Refuel: 1 cr/unit)", player.fuel, ship_info.fuel_max))
    term.move_to(2, 6)
    term.set_color(14, 0)
    if next_nav then
        term.print(string.format("Nav Computer: %s -> %s (%d cr | %d Jumps, %d Favs)", nav_info.name, next_nav.name, next_nav.price, next_nav.max_jumps, next_nav.max_favorites))
    else
        term.print(string.format("Nav Computer: %s [MAX UPGRADE]", nav_info.name))
    end

    term.render_menu("outfitter_menu", {
        holds = (player.holds + 5 <= ship_info.holds_max),
        nav = (next_nav ~= nil),
        fighters = (player.fighters < ship_info.max_fighters),
        shields = (player.shields < ship_info.max_shields),
        fuel = (player.fuel < ship_info.fuel_max),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(80, function(sub)
        if type(sub) == "string" then app.outfitter_menu(session, player) return end
        local act = sub.submit
        if act == "return" then
            if player.sector == START_SECTOR_ID then
                app.stardock_menu(session, player)
            else
                app.port_menu(session, player)
            end
            return
        end

        if act == "buy_holds" then
            if player.holds + 5 <= ship_info.holds_max and player.credits >= 450 then
                player.credits = player.credits - 450
                player.holds = player.holds + 5
                save_player(session, player)
            end
        elseif act == "buy_fighters" then
            local to_buy = math.min(10, ship_info.max_fighters - player.fighters)
            local cost = to_buy * 50
            if to_buy > 0 and player.credits >= cost then
                player.credits = player.credits - cost
                player.fighters = player.fighters + to_buy
                save_player(session, player)
            end
        elseif act == "buy_shields" then
            local to_buy = math.min(10, ship_info.max_shields - player.shields)
            local cost = to_buy * 75
            if to_buy > 0 and player.credits >= cost then
                player.credits = player.credits - cost
                player.shields = player.shields + to_buy
                save_player(session, player)
            end
        elseif act == "buy_fuel" then
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
    term.move_to(2, 1)
    term.set_color(10, 0)
    term.print("=== GALACTIC COMMERCE BANK & VAULTS ===")
    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print(string.format("Cash on Hand: %d cr    |    Vault Balance: %d cr", player.credits, player.bank or 0))

    term.render_menu("bank_menu", {
        deposit = (player.credits > 0),
        withdraw = ((player.bank or 0) > 0),
        insurance = true,
        ["return"] = true
    })
    term.flush_form()

    session.await_input(85, function(sub)
        if type(sub) == "string" then app.bank_menu(session, player) return end
        local act = sub.submit
        if act == "return" then
            if player.sector == START_SECTOR_ID then
                app.stardock_menu(session, player)
            else
                app.port_menu(session, player)
            end
            return
        end

        if act == "deposit" then
            player.bank = (player.bank or 0) + player.credits
            player.credits = 0
            save_player(session, player)
            app.bank_menu(session, player)
        elseif act == "withdraw" then
            player.credits = player.credits + (player.bank or 0)
            player.bank = 0
            save_player(session, player)
            app.bank_menu(session, player)
        elseif act == "insurance" then
            app.insurance_office(session, player)
        else
            app.bank_menu(session, player)
        end
    end)
end

function app.insurance_office(session, player)
    term.clear()
    term.move_to(2, 1)
    term.set_color(11, 0)
    term.print("=== LLOYDS OF ALPHA: INTERSTELLAR UNDERWRITERS & RESCUE ===")

    local current_tier_names = {
        [0] = "None (Uninsured - All assets lost on destruction)",
        [1] = "Bronze Policy (100% Bank Vault Protected)",
        [2] = "Silver Policy (Bank Vault + 50% Cash + Ship Hull Model)",
        [3] = "Gold Policy (Bank Vault + 100% Cash + Ship Hull + All Cargo/Upgrades)"
    }
    local curr_lvl = player.insurance_level or 0

    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print(string.format("Current Active Coverage: %s", current_tier_names[curr_lvl]))
    term.move_to(2, 4)
    term.print(string.format("Wallet: %d cr   |   Bank Vault: %d cr", player.credits, player.bank or 0))

    local rows = {
        { "Bronze", "500 cr", "100% Bank Vault Balance", "Base Scout Sloop + 100 cr" },
        { "Silver", "3,000 cr", "Bank Vault + 50% Pocket Cash", "Current Ship Class Hull + 100 cr" },
        { "Gold", "10,000 cr", "Bank Vault + 100% Cash + Cargo", "Current Ship Class + Upgrades + Holds" }
    }

    term.render_table(2, 5, {
        headers = { "Policy Tier", "Premium", "Financial Protection", "Vessel Recovery Guarantee" },
        widths = { 11, 10, 29, 32 },
        rows = rows,
        header_fg = 14,
        row_fg = 15,
        divider = true
    })

    term.render_menu("insurance_menu", {
        bronze = (curr_lvl < 1 and player.credits >= 500),
        silver = (curr_lvl < 2 and player.credits >= 3000),
        gold = (curr_lvl < 3 and player.credits >= 10000),
        cancel = true
    })
    term.flush_form()

    session.await_input(65, function(sub)
        if type(sub) == "string" then app.insurance_office(session, player) return end
        local act = sub.submit

        if act == "buy_bronze" and player.credits >= 500 then
            player.credits = player.credits - 500
            player.insurance_level = 1
            save_player(session, player)
            app.insurance_office(session, player)
        elseif act == "buy_silver" and player.credits >= 3000 then
            player.credits = player.credits - 3000
            player.insurance_level = 2
            save_player(session, player)
            app.insurance_office(session, player)
        elseif act == "buy_gold" and player.credits >= 10000 then
            player.credits = player.credits - 10000
            player.insurance_level = 3
            save_player(session, player)
            app.insurance_office(session, player)
        else
            app.stardock_menu(session, player)
        end
    end)
end

-- ---------------------------------------------------------------------------
-- STARDOCK CANTINA & TAVERN (THE SINGING PULSAR)
-- ---------------------------------------------------------------------------

local function find_profitable_trade_route()
    local sectors = get_sectors()
    local commodities = {
        { key = "ore", name = "Fuel Ore", rule_idx = 1, base = BASE_PRICES.ore },
        { key = "org", name = "Organics", rule_idx = 2, base = BASE_PRICES.org },
        { key = "eqp", name = "Equipment", rule_idx = 3, base = BASE_PRICES.eqp },
    }
    local comm = commodities[math.random(1, #commodities)]

    local sellers = {}
    local buyers = {}
    for id = 1, NUM_SECTORS do
        local sec = sectors[id]
        if sec and sec.port then
            local rules = PORT_CLASSES[sec.port.class] or {0, 0, 0}
            if rules[comm.rule_idx] == 0 then
                table.insert(sellers, id)
            elseif rules[comm.rule_idx] == 1 then
                table.insert(buyers, id)
            end
        end
    end

    if #sellers > 0 and #buyers > 0 then
        local s_id = sellers[math.random(1, #sellers)]
        local b_id = buyers[math.random(1, #buyers)]
        local buy_price = math.floor(comm.base * 0.9)
        local sell_price = math.floor(comm.base * 1.15)
        local profit = sell_price - buy_price
        local pct = math.floor((profit / buy_price) * 100)
        return {
            commodity = comm.name,
            from_sector = s_id,
            to_sector = b_id,
            buy_price = buy_price,
            sell_price = sell_price,
            profit = profit,
            pct = pct
        }
    end
    return nil
end

local function reveal_sector_radius(player, cx, cy, cz, radius)
    player.explored = player.explored or {}
    local count = 0
    for x = math.max(1, cx - radius), math.min(GRID_X, cx + radius) do
        for y = math.max(1, cy - radius), math.min(GRID_Y, cy + radius) do
            for z = math.max(1, cz - radius), math.min(GRID_Z, cz + radius) do
                local sid = to_sector_id(x, y, z)
                if sid and not player.explored[sid] then
                    player.explored[sid] = true
                    count = count + 1
                end
            end
        end
    end
    return count
end

function app.tavern_menu(session, player, status_msg)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]
    player.cantina_buzz = player.cantina_buzz or 0

    local sectors = get_sectors()
    local sec = sectors[player.sector]
    local port = sec and sec.port
    local drink_price = (player.sector == START_SECTOR_ID) and 50 or ((port and port.drink_price) or (25 + ((player.sector * 73 + 19) % 176)))
    local c_name = get_cantina_name(player.sector, port)
    local cantina_title = (player.sector == START_SECTOR_ID)
        and string.format("%s: STARDOCK TAVERN & PILOT CANTINA", string.upper(c_name))
        or string.format("%s: %s CANTINA", string.upper(c_name), string.upper((port and port.name) or ("PORT " .. player.sector)))

    term.clear()
    term.move_to(2, 1)
    term.set_color(13, 0)
    term.print("=== " .. cantina_title .. " ===")

    term.move_to(2, 3)
    term.set_color(14, 0)
    local merc_str = (player.merc_contract and player.merc_contract > 0)
        and string.format("Escort: Vanguard (%d Jumps)", player.merc_contract)
        or "Escort: None"
    term.print(string.format("Commander %s   |   Cash: %d cr   |   %s", player.nickname, player.credits, merc_str))

    term.move_to(2, 4)
    term.set_color(11, 0)
    term.print(status_msg or "Smoky neon light illuminates rugged freighter captains, smugglers, and mercenaries.")

    term.move_to(2, 6)
    term.set_color(15, 0)
    term.print("Cantina Amenities:")
    term.move_to(4, 7)
    term.print(string.format("* [D] Buy Drink (%d cr)   - Drink (+1 Buzz, Max 5: Buzz Lvl: %.1f)", drink_price, player.cantina_buzz))
    term.move_to(4, 8)
    term.print("* [H] Hear Rumors (Free)  - Rolls 0..(Buzz*10)+10 (-0.5 Buzz): Unlocks intel")
    term.move_to(4, 9)
    term.print("* [C] Buy Starcharts      - Purchase random surveyed nav charts")
    term.move_to(4, 10)
    term.print("* [P] Play Star Dice      - Wager credits in high-stakes cantina craps")
    term.move_to(4, 11)
    term.print("* [M] Hire Mercenary (400 cr) - 15-jump combat escort (+10 Firepower)")

    term.render_menu("tavern_menu", {
        round = (player.credits >= drink_price),
        rumors = true,
        charts = true,
        gamble = (player.credits >= 50),
        mercenary = (player.credits >= 400 and not (player.merc_contract and player.merc_contract > 0)),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(60, function(sub)
        if type(sub) == "string" then app.tavern_menu(session, player) return end
        local act = sub.submit
        if act == "return" then
            if player.sector == START_SECTOR_ID then
                app.stardock_menu(session, player)
            else
                app.port_menu(session, player)
            end
            return
        end

        if act == "buy_round" then
            if player.credits >= drink_price then
                player.credits = player.credits - drink_price
                player.cantina_buzz = (player.cantina_buzz or 0) + 1
                save_player(session, player)

                if player.cantina_buzz > 5 then
                    player.cantina_buzz = 0
                    local roll_penalty = math.random(1, 2)
                    local msg = ""
                    if roll_penalty == 1 then
                        local stolen = math.min(player.credits, math.max(50, math.floor(player.credits * math.random(20, 45) / 100)))
                        player.credits = player.credits - stolen
                        save_player(session, player)
                        msg = string.format("PASSED OUT: You drank too much and collapsed! Pickpockets stole %d cr. Evicted to docking bay.", stolen)
                    else
                        local fine = math.min(player.credits, 200)
                        player.credits = player.credits - fine
                        save_player(session, player)
                        msg = string.format("PASSED OUT: Station security locked you in the drunk tank! Fined %d cr and booted to station dock.", fine)
                    end
                    if player.sector == START_SECTOR_ID then
                        app.stardock_menu(session, player, msg)
                    else
                        app.port_menu(session, player, msg)
                    end
                    return
                end

                local toasts = {
                    string.format("BARTENDER: 'A round of house spirits! (Buzz Level: %.1f)'", player.cantina_buzz),
                    string.format("BARTENDER: 'Top-shelf ale poured! Spacers are chatty. (Buzz: %.1f)'", player.cantina_buzz),
                    string.format("BARTENDER: 'A generous toast, Commander! Pilots are whispering nearby. (Buzz: %.1f)'", player.cantina_buzz)
                }
                app.tavern_menu(session, player, toasts[math.random(1, #toasts)])
            else
                app.tavern_menu(session, player, string.format("BARTENDER: 'You don't have %d credits to buy a round, friend.'", drink_price))
            end
        elseif act == "listen_rumors" then
            local current_buzz = player.cantina_buzz or 0
            player.cantina_buzz = math.max(0, current_buzz - 0.5)
            save_player(session, player)

            local max_roll = math.floor(current_buzz * 10) + 10
            local roll = math.random(0, max_roll)

            if roll >= 50 then
                -- 50/50 chance of Free Merc or Free Starchart
                if math.random(1, 2) == 1 and not (player.merc_contract and player.merc_contract > 0) then
                    player.merc_contract = 15
                    save_player(session, player)
                    app.tavern_menu(session, player, "JACKPOT (Roll " .. roll .. "): A grizzled mercenary veteran offers to fly escort for 15 jumps free!")
                else
                    local rx = math.random(1, GRID_X)
                    local ry = math.random(1, GRID_Y)
                    local rz = math.random(1, GRID_Z)
                    local new_cnt = reveal_sector_radius(player, rx, ry, rz, math.random(1, 2))
                    save_player(session, player)
                    app.tavern_menu(session, player, string.format("CARTOGRAPHY DROP (Roll %d): Drunken scout dropped a datapad! Revealed %d sectors near [%02d,%02d,%d].", roll, new_cnt, rx, ry, rz))
                end
            elseif roll >= 18 then
                -- Dynamic Real Trade Route Opportunity
                local route = find_profitable_trade_route()
                if route then
                    local route_msg = string.format("HOT TRADE ROUTE (Roll %d): Buy %s at Sec %d (%d cr) -> Sell at Sec %d (%d cr) [+%d%% margin!]", roll, route.commodity, route.from_sector, route.buy_price, route.to_sector, route.sell_price, route.pct)
                    app.tavern_menu(session, player, route_msg)
                else
                    app.tavern_menu(session, player, "Trader whispers: 'Watch the market boards—Class 1 ports sell cheap Fuel Ore!'")
                end
            elseif roll >= 9 then
                -- Cosmic Anomaly: Wormhole, Asteroid, or Derelict Salvage Field!
                local sectors = get_sectors()
                local wormholes = {}
                local asteroids = {}
                local salvages = {}
                for i = 1, NUM_SECTORS do
                    local h = sectors[i].hazard
                    if h == "WORMHOLE" then table.insert(wormholes, i)
                    elseif h == "ASTEROID" or h == "ASTEROID_FIELD" then table.insert(asteroids, i)
                    elseif h == "DERELICT" or h == "DERELICT_GRAVEYARD" then table.insert(salvages, i) end
                end

                local pick_type = math.random(1, 3)
                if pick_type == 1 and #wormholes > 0 then
                    local w_sec = wormholes[math.random(1, #wormholes)]
                    player.explored = player.explored or {}
                    player.explored[w_sec] = true
                    save_player(session, player)
                    app.tavern_menu(session, player, string.format("ANOMALY INTEL (Roll %d): Sub-space scanners tracked a Wormhole Rift in Sector %d!", roll, w_sec))
                elseif pick_type == 2 and #asteroids > 0 then
                    local a_sec = asteroids[math.random(1, #asteroids)]
                    player.explored = player.explored or {}
                    player.explored[a_sec] = true
                    save_player(session, player)
                    app.tavern_menu(session, player, string.format("MINING TIP (Roll %d): Scout confirms a rich Asteroid Belt in Sector %d with free fuel ore!", roll, a_sec))
                elseif #salvages > 0 then
                    local s_sec = salvages[math.random(1, #salvages)]
                    player.explored = player.explored or {}
                    player.explored[s_sec] = true
                    save_player(session, player)
                    app.tavern_menu(session, player, string.format("SALVAGE RADAR (Roll %d): Derelict fleet graveyard located in Sector %d! High-value salvage potential.", roll, s_sec))
                else
                    app.tavern_menu(session, player, "Bounty hunter: 'Beware of deep void sectors; pirate corsairs lurk outside federation space.'")
                end
            else
                -- Lore & Pirate Warnings
                local rumors = {
                    "Overheard: 'Federation patrol wiped out a pirate clan near the Orion reach.'",
                    "Overheard: 'Class 0 ports are rare anomalies hidden in deep void sectors.'",
                    "Spacer whispers: 'Always keep at least 15 shield units before jumping into storms.'",
                    "Drunk pilot boasts: 'Upgraded my Nav Computer to Mark IV—jumps 50 sectors in one hop!'",
                    "Bar patron: 'Void Trader expanse holds 2,250 charted sectors. Nobody has seen them all.'"
                }
                app.tavern_menu(session, player, rumors[math.random(1, #rumors)])
            end
        elseif act == "buy_charts" then
            app.nav_charts_menu(session, player)
        elseif act == "hire_merc" then
            if player.credits >= 400 then
                player.credits = player.credits - 400
                player.merc_contract = 15
                save_player(session, player)
                app.tavern_menu(session, player, "CONTRACT SIGNED: Vanguard Mercenary Wing hired for 15 jumps (+10 Firepower).")
            else
                app.tavern_menu(session, player, "MERCENARY: 'My contract rate is 400 credits upfront, Commander.'")
            end
        elseif act == "gamble_dice" then
            app.dice_game(session, player, "Roll 2d6 against the Cantina dealer! Higher roll wins 2x your bet.")
        else
            app.tavern_menu(session, player)
        end
    end)
end

function app.nav_charts_menu(session, player, chart_msg)
    player.explored = player.explored or {}
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    if not session.cantina_charts then
        local num_charts = math.random(1, 3)
        local generated = {}
        for i = 1, num_charts do
            local rx = math.random(1, GRID_X)
            local ry = math.random(1, GRID_Y)
            local rz = math.random(1, GRID_Z)
            local r = math.random(1, 3)

            local x_min = math.max(1, rx - r)
            local x_max = math.min(GRID_X, rx + r)
            local y_min = math.max(1, ry - r)
            local y_max = math.min(GRID_Y, ry + r)
            local z_min = math.max(1, rz - r)
            local z_max = math.min(GRID_Z, rz + r)
            local system_count = (x_max - x_min + 1) * (y_max - y_min + 1) * (z_max - z_min + 1)

            local dist = math.sqrt((rx - cur_x)^2 + (ry - cur_y)^2 + (rz - cur_z)^2)
            local raw_cost = math.floor((system_count * 80) * (1 + 0.10 * dist))

            local has_deal = (math.random(1, 10) == 1)
            local discount_pct = has_deal and math.random(50, 70) or 0
            local final_cost = has_deal and math.floor(raw_cost * (1 - (discount_pct / 100))) or raw_cost

            local titles = {
                [1] = "Sector Cluster Recon",
                [2] = "Quadrant Survey Map",
                [3] = "Deep Void Galactic Atlas"
            }

            table.insert(generated, {
                title = titles[r] or "Survey Chart",
                radius = r,
                x = rx,
                y = ry,
                z = rz,
                systems = system_count,
                dist = dist,
                cost = final_cost,
                discount_pct = discount_pct
            })
        end
        session.cantina_charts = generated
    end

    local charts = session.cantina_charts

    term.clear()
    term.move_to(2, 1)
    term.set_color(13, 0)
    term.print("=== CANTINA CARTOGRAPHY EXCHANGE: SURVEYED STAR CHARTS ===")

    term.move_to(2, 3)
    term.set_color(14, 0)
    term.print(string.format("Commander %s   |   Cash: %d cr", player.nickname, player.credits))

    term.move_to(2, 4)
    term.set_color(11, 0)
    term.print(chart_msg or "Available navigational surveys in this cantina:")

    local rows = {}
    for idx, c in ipairs(charts) do
        local disc_tag = (c.discount_pct > 0) and string.format(" [DEAL -%d%%!]", c.discount_pct) or ""
        local size_tag = string.format("Rad %d (%d systems)", c.radius, c.systems)
        local loc_tag = string.format("[%02d,%02d,%d]", c.x, c.y, c.z)
        table.insert(rows, {
            string.format("[%d] %s", idx, c.title),
            size_tag,
            loc_tag,
            string.format("%d cr%s", c.cost, disc_tag)
        })
    end

    term.render_table(2, 6, {
        headers = { "Survey Title", "Coverage Area", "Sector Focus", "Price" },
        widths = { 26, 20, 14, 18 },
        rows = rows,
        header_fg = 14,
        row_fg = 15,
        divider = true
    })

    local c1 = charts[1]
    local c2 = charts[2]
    local c3 = charts[3]

    term.render_menu("chart_menu", {
        chart1 = (c1 ~= nil and player.credits >= c1.cost),
        chart2 = (c2 ~= nil and player.credits >= c2.cost),
        chart3 = (c3 ~= nil and player.credits >= c3.cost),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(50, function(sub)
        if type(sub) == "string" then app.nav_charts_menu(session, player) return end
        local act = sub.submit
        if act == "return" then
            app.tavern_menu(session, player)
            return
        end

        local chosen_idx = 0
        if act == "buy_chart1" then chosen_idx = 1
        elseif act == "buy_chart2" then chosen_idx = 2
        elseif act == "buy_chart3" then chosen_idx = 3 end

        if chosen_idx >= 1 and chosen_idx <= #charts then
            local c = charts[chosen_idx]
            if player.credits >= c.cost then
                player.credits = player.credits - c.cost
                local new_sectors = reveal_sector_radius(player, c.x, c.y, c.z, c.radius)
                save_player(session, player)
                session.cantina_charts = nil
                app.nav_charts_menu(session, player, string.format("CHART PURCHASED: Ingested telemetry! Revealed %d new sectors on your starchart.", new_sectors))
            else
                app.nav_charts_menu(session, player, "Insufficient credits for that navigational chart!")
            end
        else
            app.nav_charts_menu(session, player)
        end
    end)
end

function app.dice_game(session, player, dice_msg)
    term.clear()
    term.move_to(2, 1)
    term.set_color(13, 0)
    term.print("=== CANTINA STAR DICE (2D6 HIGH ROLLER) ===")

    term.move_to(2, 3)
    term.set_color(14, 0)
    term.print(string.format("Commander %s   |   Cash: %d cr", player.nickname, player.credits))

    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print(dice_msg or "Select your bet size to roll:")

    term.render_menu("dice_menu", {
        bet_low = (player.credits >= 50),
        bet_mid = (player.credits >= 200),
        bet_high = (player.credits >= 500),
        ["return"] = true
    })
    term.flush_form()

    session.await_input(45, function(sub)
        if type(sub) == "string" then app.dice_game(session, player) return end
        local act = sub.submit
        if act == "return" then
            app.tavern_menu(session, player, "Stepped away from the dice tables.")
            return
        end

        local bet = 0
        if act == "bet_50" and player.credits >= 50 then bet = 50
        elseif act == "bet_200" and player.credits >= 200 then bet = 200
        elseif act == "bet_500" and player.credits >= 500 then bet = 500
        end

        if bet > 0 then
            local p_roll = math.random(1, 6) + math.random(1, 6)
            local d_roll = math.random(1, 6) + math.random(1, 6)
            if p_roll > d_roll then
                player.credits = player.credits + bet
                save_player(session, player)
                app.dice_game(session, player, string.format("WIN! You rolled %d vs Dealer's %d. Won +%d cr!", p_roll, d_roll, bet))
            elseif p_roll < d_roll then
                player.credits = player.credits - bet
                save_player(session, player)
                app.dice_game(session, player, string.format("LOSS! You rolled %d vs Dealer's %d. Lost -%d cr.", p_roll, d_roll, bet))
            else
                app.dice_game(session, player, string.format("PUSH! Both rolled %d. Bet returned.", p_roll))
            end
        else
            app.dice_game(session, player, "Insufficient credits for that bet size!")
        end
    end)
end

-- ---------------------------------------------------------------------------
-- SENSORS, STATUS, & LEADERBOARD (USING TABLE API)
-- ---------------------------------------------------------------------------

function app.scan_sector(session, player)
    local sectors = get_sectors()
    local sec = sectors[player.sector]
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.move_to(2, 1)
    term.set_color(11, 0)
    term.print(string.format("=== LONG RANGE SCAN (Sector %d [%02d,%02d,%d]) ===", player.sector, cur_x, cur_y, cur_z))

    if sec.hazard == "COSMIC_STORM" then
        term.move_to(2, 3)
        term.set_color(12, 0)
        term.print(">>> SENSOR ARRAY SCRAMBLED BY IONIZED STORM INTERFERENCE <<<")
        term.move_to(2, 4)
        term.set_color(8, 0)
        term.print("Sensor telemetry offline. Clear nebula to restore LRS feed.")
    else
        player.explored = player.explored or {}
        local rows = {}
        for _, dest in ipairs(sec.warps or {}) do
            player.explored[dest] = true
            local d_sec = sectors[dest]
            local dx, dy, dz = to_coords(dest)
            local dir_name, dir_code = get_direction_name(player.sector, dest)
            local port_str = "Deep Space"
            if dest == START_SECTOR_ID then
                port_str = "Stardock [Cantina]"
            elseif d_sec and d_sec.port then
                local fav_tag = is_favorite_sector(player, dest) and " [*]" or ""
                local has_c = d_sec.port.has_cantina or ((dest * 37 + 13) % 100 < 5)
                local c_tag = has_c and " [Cantina]" or ""
                local p_info = PORT_CLASSES[d_sec.port.class] or { code = "???" }
                local s_tier = get_port_services(dest, d_sec.port)
                local p_code = string.format("%s%s", p_info.code or "???", s_tier)
                port_str = string.format("Class %d [%s]%s%s", d_sec.port.class, p_code, c_tag, fav_tag)
            end
            local hazard_str = (d_sec and d_sec.hazard) or "Clear"

            table.insert(rows, {
                string.format("[%s] %s", dir_code, dir_name),
                tostring(dest),
                string.format("[%02d,%02d,%d]", dx, dy, dz),
                port_str,
                hazard_str
            })
        end

        term.render_table(2, 3, {
            headers = { "Vector", "Sector", "Coords", "Port Status", "Hazards" },
            widths = { 9, 8, 12, 20, 16 },
            rows = rows,
            header_fg = 14,
            row_fg = 15,
            divider = true
        })
    end

    term.define_form(90)
    term.add_submit_button("back", 2, 10)
    term.flush_form()

    session.await_input(90, function() app.view_sector(session, player, "") end)
end

function app.view_status(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]
    local nav_info = NAV_COMPUTERS[player.nav_level or 1] or NAV_COMPUTERS[1]
    local cur_x, cur_y, cur_z = to_coords(player.sector)

    term.clear()
    term.move_to(2, 1)
    term.set_color(14, 0)
    term.print("=== COMMANDER DOSSIER & MANIFEST ===")

    term.move_to(2, 3)
    term.set_color(15, 0)
    term.print(string.format("Commander: %-15s   Ship Class: %s", player.nickname, ship_info.name))
    term.move_to(2, 4)
    term.print(string.format("Location:  Sector %d [%02d,%02d,%d]   Turns: %d/%d   Fuel: %d/%d", player.sector, cur_x, cur_y, cur_z, player.turns, MAX_TURNS, player.fuel, ship_info.fuel_max))
    term.move_to(2, 5)
    term.print(string.format("Cash:      %-10d cr   Bank Vault: %d cr      Nav: %s", player.credits, player.bank or 0, nav_info.name))
    term.move_to(2, 6)
    term.print(string.format("Net Worth: %-10d cr   Pirates Vanquished: %d", calc_net_worth(player), player.kills or 0))

    term.move_to(2, 7)
    term.set_color(11, 0)
    term.print(string.format("Cargo Hold Manifest: %d / %d bays utilized", (player.ore + player.org + player.eqp), player.holds))

    local avg_ore = get_cargo_avg_cost(player, "ore")
    local avg_org = get_cargo_avg_cost(player, "org")
    local avg_eqp = get_cargo_avg_cost(player, "eqp")

    term.move_to(2, 8)
    term.print(string.format("  * Fuel Ore:  %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.ore, avg_ore, player.ore_cost or 0))
    term.move_to(2, 9)
    term.print(string.format("  * Organics:  %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.org, avg_org, player.org_cost or 0))
    term.move_to(2, 10)
    term.print(string.format("  * Equipment: %-5d units (Avg Paid: %-3d cr/unit | Basis: %d cr)", player.eqp, avg_eqp, player.eqp_cost or 0))

    term.define_form(95)
    term.add_submit_button("back", 2, 12)
    term.flush_form()

    session.await_input(95, function() app.view_sector(session, player, "") end)
end

function app.view_leaderboard(session, player, from_entry)
    local board = db.get("vt_leaderboard", "scores") or {}
    term.clear()
    term.move_to(2, 1)
    term.set_color(14, 0)
    term.print("=== GALACTIC HALL OF FAME ===")

    local rows = {}
    for i = 1, math.min(10, math.max(1, #board)) do
        if board[i] then
            local e = board[i]
            table.insert(rows, {
                string.format("#%d", i),
                e.nickname or "Unknown",
                e.ship or "Scout",
                tostring(e.sector or START_SECTOR_ID),
                tostring(e.kills or 0),
                string.format("%d cr", e.net_worth or 0)
            })
        else
            table.insert(rows, { string.format("#%d", i), "---", "---", "-", "-", "-" })
        end
    end

    term.render_table(2, 3, {
        headers = { "Rank", "Commander", "Vessel Class", "Sector", "Kills", "Net Worth" },
        widths = { 6, 18, 18, 8, 7, 12 },
        rows = rows,
        header_fg = 11,
        row_fg = 15,
        divider = true
    })

    term.define_form(99)
    term.add_submit_button("back", 2, 12)
    term.flush_form()

    session.await_input(99, function()
        if from_entry then
            app.on_start(session)
        else
            app.view_sector(session, player, "")
        end
    end)
end

return app
