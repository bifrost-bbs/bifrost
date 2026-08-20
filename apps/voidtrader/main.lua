-- Void Trader: Interstellar Frontier Trading Strategy Door Game
-- An homage to classic BBS space trading games (e.g. TradeWars 2002)

local app = {}

local NUM_SECTORS = 100
local MAX_TURNS = 120

-- Ship Classes: name, holds_base, holds_max, max_fighters, max_shields, price
local SHIP_CLASSES = {
    { name = "Scout Sloop", holds_base = 20, holds_max = 35, max_fighters = 25, max_shields = 25, price = 0 },
    { name = "Merchant Hauler", holds_base = 50, holds_max = 80, max_fighters = 60, max_shields = 60, price = 4500 },
    { name = "Armored Freighter", holds_base = 100, holds_max = 150, max_fighters = 120, max_shields = 120, price = 14000 },
    { name = "Dreadnought Cruiser", holds_base = 200, holds_max = 300, max_fighters = 250, max_shields = 250, price = 42000 }
}

-- Port Classes: [Ore, Org, Eqp] where 1 = Port BUYS (Player Sells), 0 = Port SELLS (Player Buys)
-- Class 0 is Stardock (Sector 1 only: Special Central Hub)
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
-- UNIVERSE & PLAYER DATABASE MANAGEMENT
-- ---------------------------------------------------------------------------

local function init_universe()
    local sectors = {}
    for i = 1, NUM_SECTORS do
        local num_warps = math.random(2, 5)
        local warps = {}
        for _ = 1, num_warps do
            local dest = math.random(1, NUM_SECTORS)
            if dest ~= i then
                local exists = false
                for _, w in ipairs(warps) do
                    if w == dest then exists = true break end
                end
                if not exists then table.insert(warps, dest) end
            end
        end

        local port = nil
        if i == 1 then
            -- Sector 1 is Stardock Central Starbase
            port = { class = 0, name = "Alpha Stardock Prime", ore = 9999, org = 9999, eqp = 9999 }
        elseif math.random(1, 100) <= 75 then
            local p_class = math.random(1, 8)
            port = {
                class = p_class,
                name = "Port " .. string.char(64 + p_class) .. "-" .. i,
                ore = math.random(600, 3000),
                org = math.random(400, 2500),
                eqp = math.random(200, 1500)
            }
        end

        -- Sector hazards and anomaly events
        local hazard = nil
        local roll = math.random(1, 100)
        if i > 1 and roll <= 8 then
            hazard = "ASTEROID_FIELD"
        elseif i > 1 and roll <= 14 then
            hazard = "COSMIC_STORM"
        end

        sectors[i] = {
            id = i,
            warps = warps,
            port = port,
            hazard = hazard,
            defense_fighters = 0,
            defense_owner = nil
        }
    end

    -- Ensure Sector 1 has fixed bidirectional warps for exploration
    sectors[1].warps = { 2, 3, 4, 5 }
    for _, dest in ipairs({ 2, 3, 4, 5 }) do
        local has_back = false
        for _, w in ipairs(sectors[dest].warps) do
            if w == 1 then has_back = true break end
        end
        if not has_back then table.insert(sectors[dest].warps, 1) end
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
        sector = 1,
        credits = 1200,
        bank = 0,
        turns = MAX_TURNS,
        ship_class = 1, -- Scout Sloop
        holds = 20,
        ore = 0,
        org = 0,
        eqp = 0,
        fighters = 15,
        shields = 15,
        kills = 0,
        trades = 0,
        last_turn_day = os.date("%Y%m%d")
    }
end

local function get_player(session)
    local p = db.get("vt_players", session.node_id())
    if not p or type(p) ~= "table" then
        p = init_player(session)
        db.set("vt_players", session.node_id(), p)
    end

    -- Refresh user nickname if updated
    local user = db.get("users", session.node_id()) or {}
    if user.nickname then p.nickname = user.nickname end

    -- Daily Turn Replenishment
    local today = os.date("%Y%m%d")
    if p.last_turn_day ~= today or p.turns <= 0 then
        p.turns = MAX_TURNS
        p.last_turn_day = today
        db.set("vt_players", session.node_id(), p)
    end

    return p
end

local function calc_net_worth(p)
    local ship_info = SHIP_CLASSES[p.ship_class or 1] or SHIP_CLASSES[1]
    local cargo_val = (p.ore * BASE_PRICES.ore) + (p.org * BASE_PRICES.org) + (p.eqp * BASE_PRICES.eqp)
    local ship_val = ship_info.price + ((p.holds - ship_info.holds_base) * 100)
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

    -- Sort top 10 descending by net worth
    table.sort(board, function(a, b) return (a.net_worth or 0) > (b.net_worth or 0) end)
    while #board > 15 do table.remove(board) end
    db.set("vt_leaderboard", "scores", board)
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
    local sec = sectors[player.sector] or sectors[1]
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(14, 0) -- Yellow
    term.print(string.format("Sector: %-3d   Turns: %-3d   Credits: %-7d cr   Bank: %-7d cr", player.sector, player.turns, player.credits, player.bank or 0))

    term.move_to(2, 7)
    term.set_color(11, 0) -- Cyan
    term.print(string.format("Ship: %-18s Holds: %d/%-3d  Fighters: %-3d  Shields: %-3d", ship_info.name, (player.ore + player.org + player.eqp), player.holds, player.fighters, player.shields))

    term.move_to(2, 8)
    term.set_color(15, 0) -- White
    local warp_str = ""
    for _, w in ipairs(sec.warps or {}) do
        warp_str = warp_str .. tostring(w) .. " "
    end
    term.print("Warp Lanes: " .. warp_str)

    term.move_to(2, 9)
    if player.sector == 1 then
        term.set_color(10, 0) -- Green
        term.print("Facilities: [ Alpha Stardock Prime ] (Shipyard, Bank, Outfitter, Lounge)")
    elseif sec.port then
        term.set_color(10, 0)
        local p_info = PORT_CLASSES[sec.port.class] or { name = "Trading Station" }
        term.print(string.format("Port: Class %d - %s", sec.port.class, p_info.name))
    else
        term.set_color(8, 0) -- Grey
        term.print("Port: None in this sector (Deep Space)")
    end

    if sec.hazard then
        term.move_to(2, 10)
        term.set_color(12, 0) -- Bright Red
        term.print("HAZARD DETECTED: " .. sec.hazard)
    end

    if (sec.defense_fighters or 0) > 0 then
        term.move_to(2, 10)
        term.set_color(13, 0)
        term.print(string.format("Sector Guard: %d Combat Drones deployed by %s", sec.defense_fighters, sec.defense_owner or "Unknown"))
    end

    term.move_to(2, 11)
    term.set_color(15, 0)
    term.print(msg or "")

    term.define_form(10)
    term.add_submit_button("warp", 2, 13)
    if player.sector == 1 then
        term.add_submit_button("stardock", 12, 13)
    elseif sec.port then
        term.add_submit_button("dock", 12, 13)
    end
    term.add_submit_button("scan", 22, 13)
    term.add_submit_button("status", 32, 13)
    term.add_submit_button("ranks", 42, 13)
    term.add_submit_button("exit", 52, 13)
    term.flush_form()

    session.await_input(10, function(sub)
        if type(sub) == "string" then app.view_sector(session, player, "") return end
        local act = sub.submit

        if act == "warp" then
            app.nav_prompt(session, player)
        elseif act == "dock" then
            app.port_menu(session, player)
        elseif act == "stardock" then
            app.stardock_menu(session, player)
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
-- NAVIGATION & WARPING
-- ---------------------------------------------------------------------------

function app.nav_prompt(session, player)
    local sectors = get_sectors()
    local sec = sectors[player.sector]

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(11, 0)
    term.print(string.format("Current Sector: %d (Turns remaining: %d)", player.sector, player.turns))
    term.move_to(2, 7)
    local warp_str = ""
    for _, w in ipairs(sec.warps or {}) do warp_str = warp_str .. tostring(w) .. " " end
    term.print("Available Warp Coordinates: " .. warp_str)

    term.define_form(20)
    term.move_to(2, 9)
    term.set_color(15, 0)
    term.print("Target Sector: ")
    term.add_input_field("dest", 18, 9, 6, "")
    term.add_submit_button("engage", 2, 11)
    term.add_submit_button("cancel", 14, 11)
    term.flush_form()

    session.await_input(20, function(sub)
        if type(sub) == "string" then app.nav_prompt(session, player) return end
        if sub.submit == "cancel" then
            app.view_sector(session, player, "Warp aborted.")
            return
        end

        local dest = tonumber(sub.dest)
        if not dest or dest < 1 or dest > NUM_SECTORS then
            app.view_sector(session, player, "Invalid sector coordinate.")
            return
        end

        if player.turns <= 0 then
            app.view_sector(session, player, "Hyperspace engine offline: Out of turns!")
            return
        end

        local valid = false
        for _, w in ipairs(sec.warps or {}) do
            if w == dest then valid = true break end
        end

        if not valid then
            app.view_sector(session, player, "No direct hyperspace lane to Sector " .. dest .. "!")
            return
        end

        player.sector = dest
        player.turns = player.turns - 1
        save_player(session, player)

        local dest_sec = sectors[dest]

        -- Sector Hazard check
        if dest_sec.hazard == "ASTEROID_FIELD" then
            if player.shields > 0 then
                local dmg = math.min(player.shields, math.random(2, 5))
                player.shields = player.shields - dmg
                save_player(session, player)
            end
        end

        -- Random Encounters (Pirate ambush or Derelict salvage)
        local roll = math.random(1, 100)
        if dest > 1 and roll <= 18 then
            app.pirate_encounter(session, player, math.random(8, 25))
        elseif dest > 1 and roll <= 28 then
            app.derelict_salvage(session, player)
        else
            app.view_sector(session, player, "Warp jump successful: Arrived in Sector " .. dest .. ".")
        end
    end)
end

-- ---------------------------------------------------------------------------
-- COMBAT & SPACE ENCOUNTERS
-- ---------------------------------------------------------------------------

function app.pirate_encounter(session, player, enemy_fighters)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 6)
    term.set_color(12, 0) -- Bright Red
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

            -- Shields absorb damage first
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
            -- Tactical defensive posture: recharge shields slightly or absorb hit
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
            if math.random(1, 100) <= 60 then
                player.turns = math.max(0, player.turns - 1)
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
    term.print("══════════════════════════════════════════════════════════")
    term.move_to(2, 7)
    term.print("                   VESSEL DESTROYED                       ")
    term.move_to(2, 8)
    term.print("══════════════════════════════════════════════════════════")
    term.move_to(2, 10)
    term.set_color(15, 0)
    term.print(cause)
    term.move_to(2, 12)
    term.set_color(14, 0)
    term.print("Escape pod retrieved by Alpha Stardock rescue team.")
    term.move_to(2, 13)
    term.print("Your bank account credits remained secure.")

    -- Reset to starter ship in Sector 1 while preserving bank balance
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
-- COMMODITY TRADING AT STARPORTS
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

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print(string.format("=== %s (Class %d) ===", port.name or "Commerce Post", port.class))

    term.move_to(2, 7)
    term.set_color(14, 0)
    term.print("Commodity    Port Supply   In Cargo   Station Action   Market Price")
    term.move_to(2, 8)
    term.print("─────────    ───────────   ────────   ──────────────   ────────────")

    local function port_item(y, name, port_amt, pl_amt, rule, base_p)
        term.move_to(2, y)
        local act = rule == 1 and "BUYING" or "SELLING"
        local color = rule == 1 and 10 or 11
        local price = rule == 1 and math.floor(base_p * 0.9) or math.floor(base_p * 1.15)
        term.set_color(color, 0)
        term.print(string.format("%-11s  %-11d   %-8d   %-14s   %d cr/unit", name, port_amt, pl_amt, act, price))
        return price
    end

    local pr_ore = port_item(9, "Fuel Ore", port.ore, player.ore, p_rules[1], BASE_PRICES.ore)
    local pr_org = port_item(10, "Organics", port.org, player.org, p_rules[2], BASE_PRICES.org)
    local pr_eqp = port_item(11, "Equipment", port.eqp, player.eqp, p_rules[3], BASE_PRICES.eqp)

    term.move_to(2, 13)
    term.set_color(15, 0)
    local holds_used = player.ore + player.org + player.eqp
    term.print(string.format("Cash: %-8d cr   Holds Free: %d / %d", player.credits, (player.holds - holds_used), player.holds))

    term.define_form(50)
    term.add_submit_button("trade_ore", 2, 15)
    term.add_submit_button("trade_org", 16, 15)
    term.add_submit_button("trade_eqp", 30, 15)
    term.add_submit_button("depart", 44, 15)
    term.flush_form()

    session.await_input(50, function(sub)
        if type(sub) == "string" then app.port_menu(session, player) return end
        local act = sub.submit

        if act == "depart" then
            app.view_sector(session, player, "Departed starport.")
            return
        end

        local function execute_trade(item_key, rule, price, port_amt, pl_amt, item_name)
            local free_h = player.holds - (player.ore + player.org + player.eqp)
            if rule == 1 then
                -- Station buys from player -> player sells cargo
                if pl_amt > 0 then
                    local total_val = pl_amt * price
                    player.credits = player.credits + total_val
                    player[item_key] = 0
                    port[item_key] = port[item_key] + pl_amt
                    player.trades = (player.trades or 0) + 1
                    save_player(session, player)
                    save_sectors(sectors)
                    app.port_menu(session, player)
                else
                    app.view_sector(session, player, "You do not have any " .. item_name .. " in cargo.")
                end
            else
                -- Station sells to player -> player buys cargo
                if free_h <= 0 then
                    app.view_sector(session, player, "Cargo bays full! Upgrade holds at Stardock.")
                    return
                end
                local max_afford = math.floor(player.credits / price)
                local buy_qty = math.min(free_h, max_afford, port_amt)
                if buy_qty > 0 then
                    player.credits = player.credits - (buy_qty * price)
                    player[item_key] = player[item_key] + buy_qty
                    port[item_key] = port[item_key] - buy_qty
                    player.trades = (player.trades or 0) + 1
                    save_player(session, player)
                    save_sectors(sectors)
                    app.port_menu(session, player)
                else
                    app.view_sector(session, player, "Insufficient credits or out of station stock.")
                end
            end
        end

        if act == "trade_ore" then
            execute_trade("ore", p_rules[1], pr_ore, port.ore, player.ore, "Fuel Ore")
        elseif act == "trade_org" then
            execute_trade("org", p_rules[2], pr_org, port.org, player.org, "Organics")
        elseif act == "trade_eqp" then
            execute_trade("eqp", p_rules[3], pr_eqp, port.eqp, player.eqp, "Equipment")
        end
    end)
end

-- ---------------------------------------------------------------------------
-- SECTOR 1: CENTRAL STARDOCK (SHIPYARD, BANK, OUTFITTER)
-- ---------------------------------------------------------------------------

function app.stardock_menu(session, player)
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print("══════════════════════════════════════════════════════════════")
    term.move_to(2, 6)
    term.print("               ALPHA STARDOCK PRIME - CENTRAL HUB              ")
    term.move_to(2, 7)
    term.print("══════════════════════════════════════════════════════════════")

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
            app.view_sector(session, player, "Launched into Sector 1.")
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
    term.print("Ship Class            Max Holds   Combat Drones   Shields   Price")
    term.move_to(2, 8)
    term.print("──────────            ─────────   ─────────────   ───────   ─────")

    for i, ship in ipairs(SHIP_CLASSES) do
        term.move_to(2, 8 + i)
        local is_curr = (player.ship_class or 1) == i
        term.set_color(is_curr and 10 or 15, 0)
        local marker = is_curr and "[OWNED]" or string.format("%d cr", ship.price)
        term.print(string.format("%-20s  %-9d   %-13d   %-7d   %s", ship.name, ship.holds_max, ship.max_fighters, ship.max_shields, marker))
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
            save_player(session, player)
            app.stardock_menu(session, player)
        else
            app.stardock_menu(session, player)
        end
    end)
end

function app.outfitter_menu(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1]
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print("=== NAVAL OUTFITTER & ARMORY ===")
    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(string.format("Cargo Holds: %d / %d max (Upgrade: 450 cr / +5 holds)", player.holds, ship_info.holds_max))
    term.move_to(2, 8)
    term.print(string.format("Combat Drones / Fighters: %d / %d max (50 cr each)", player.fighters, ship_info.max_fighters))
    term.move_to(2, 9)
    term.print(string.format("Deflector Shields: %d / %d max (75 cr each)", player.shields, ship_info.max_shields))

    term.define_form(80)
    term.add_submit_button("buy_5holds", 2, 12)
    term.add_submit_button("buy_10fighters", 18, 12)
    term.add_submit_button("buy_10shields", 36, 12)
    term.add_submit_button("back", 52, 12)
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

    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(11, 0)
    term.print(string.format("=== LONG RANGE SCAN (Sector %d) ===", player.sector))

    term.move_to(2, 7)
    term.set_color(14, 0)
    term.print("Target   Port Status           Hazards          Adjacent Warps")
    term.move_to(2, 8)
    term.print("──────   ───────────           ───────          ──────────────")

    for idx, dest in ipairs(sec.warps or {}) do
        term.move_to(2, 8 + idx)
        local d_sec = sectors[dest]
        local port_str = "Deep Space (None)"
        if dest == 1 then
            port_str = "Stardock Prime"
        elseif d_sec and d_sec.port then
            port_str = string.format("Class %d (%s)", d_sec.port.class, d_sec.port.name or "Port")
        end
        local hazard_str = (d_sec and d_sec.hazard) or "Clear"
        local w_list = ""
        for _, w in ipairs((d_sec and d_sec.warps) or {}) do w_list = w_list .. tostring(w) .. " " end

        term.set_color(15, 0)
        term.print(string.format("Sec %-3d  %-20s  %-15s  %s", dest, port_str, hazard_str, w_list))
    end

    term.define_form(90)
    term.add_submit_button("back", 2, 16)
    term.flush_form()

    session.await_input(90, function() app.view_sector(session, player, "") end)
end

function app.view_status(session, player)
    local ship_info = SHIP_CLASSES[player.ship_class or 1] or SHIP_CLASSES[1]
    term.clear()
    term.render_asset("voidtrader_banner")
    term.move_to(2, 5)
    term.set_color(14, 0)
    term.print("=== COMMANDER DOSSIER & MANIFEST ===")

    term.move_to(2, 7)
    term.set_color(15, 0)
    term.print(string.format("Commander: %-15s   Ship Class: %s", player.nickname, ship_info.name))
    term.move_to(2, 8)
    term.print(string.format("Location:  Sector %-8d   Turns Left: %d / %d", player.sector, player.turns, MAX_TURNS))
    term.move_to(2, 9)
    term.print(string.format("Cash:      %-10d cr   Bank Vault: %d cr", player.credits, player.bank or 0))
    term.move_to(2, 10)
    term.print(string.format("Net Worth: %-10d cr   Pirates Vanquished: %d", calc_net_worth(player), player.kills or 0))

    term.move_to(2, 12)
    term.set_color(11, 0)
    term.print(string.format("Cargo Hold Manifest: %d / %d bays utilized", (player.ore + player.org + player.eqp), player.holds))
    term.move_to(2, 13)
    term.print(string.format("  • Fuel Ore:  %-5d units   (Est. Value: %d cr)", player.ore, player.ore * BASE_PRICES.ore))
    term.move_to(2, 14)
    term.print(string.format("  • Organics:  %-5d units   (Est. Value: %d cr)", player.org, player.org * BASE_PRICES.org))
    term.move_to(2, 15)
    term.print(string.format("  • Equipment: %-5d units   (Est. Value: %d cr)", player.eqp, player.eqp * BASE_PRICES.eqp))

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
    term.print("────   ─────────           ────────────        ──────   ─────   ─────────")

    for i = 1, math.min(10, math.max(1, #board)) do
        term.move_to(2, 8 + i)
        if board[i] then
            local e = board[i]
            local is_me = e.node_id == session.node_id()
            term.set_color(is_me and 10 or 15, 0)
            term.print(string.format("#%-4d  %-18s  %-18s  %-7d  %-5d   %d cr", i, e.nickname or "Unknown", e.ship or "Scout", e.sector or 1, e.kills or 0, e.net_worth or 0))
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
