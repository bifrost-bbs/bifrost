local app = {}

local NUM_SECTORS = 100
local MAX_TURNS = 100

-- Port Classes: Buy/Sell limits and types
-- O = Fuel Ore, G = Organics, E = Equipment
-- 1 = Buy, 0 = Sell (relative to player, so port BUYS if it's 1, player gets credits)
-- Class 1: Buys Ore, Sells Org, Sells Eqp (1, 0, 0)
-- Class 2: Sells Ore, Buys Org, Sells Eqp (0, 1, 0)
-- Class 3: Sells Ore, Sells Org, Buys Eqp (0, 0, 1)
-- Class 4: Sells Ore, Buys Org, Buys Eqp (0, 1, 1)
-- Class 5: Buys Ore, Sells Org, Buys Eqp (1, 0, 1)
-- Class 6: Buys Ore, Buys Org, Sells Eqp (1, 1, 0)
-- Class 7: Buys Ore, Buys Org, Buys Eqp (1, 1, 1)
-- Class 8: Sells Ore, Sells Org, Sells Eqp (0, 0, 0)
local PORT_CLASSES = {
    {1, 0, 0}, {0, 1, 0}, {0, 0, 1},
    {0, 1, 1}, {1, 0, 1}, {1, 1, 0},
    {1, 1, 1}, {0, 0, 0}
}

local function init_universe()
    local sectors = {}
    for i = 1, NUM_SECTORS do
        local num_warps = math.random(2, 6)
        local warps = {}
        for j = 1, num_warps do
            local dest = math.random(1, NUM_SECTORS)
            if dest ~= i then
                local exists = false
                for _, w in ipairs(warps) do
                    if w == dest then exists = true end
                end
                if not exists then table.insert(warps, dest) end
            end
        end

        local port = nil
        if math.random(1, 100) > 30 or i == 1 then -- 70% chance of a port
            local p_class = math.random(1, 8)
            port = {
                class = p_class,
                ore = math.random(500, 2500),
                org = math.random(500, 2500),
                eqp = math.random(500, 2500)
            }
        end

        sectors[i] = {
            warps = warps,
            port = port
        }
    end

    -- Ensure sector 1 has some warps
    if #sectors[1].warps == 0 then
        sectors[1].warps = {2, 3, 4}
    end

    db.set("tw_sectors", "all", sectors)
    return sectors
end

local function init_player(session)
    return {
        sector = 1,
        credits = 1000,
        turns = MAX_TURNS,
        holds = 20,
        ore = 0,
        org = 0,
        eqp = 0,
        fighters = 10,
        shields = 0
    }
end

local function get_sectors()
    local s = db.get("tw_sectors", "all")
    if not s then s = init_universe() end
    return s
end

local function get_player(session)
    local p = db.get("tw_players", session.node_id())
    if not p or type(p) ~= "table" then
        p = init_player(session)
        db.set("tw_players", session.node_id(), p)
    end
    -- Replenish turns if needed (simple daily logic could be added, just max it out for now if 0 and starting app)
    if p.turns <= 0 then
        p.turns = MAX_TURNS
    end
    return p
end

local function save_player(session, player)
    db.set("tw_players", session.node_id(), player)
end

local function save_sectors(sectors)
    db.set("tw_sectors", "all", sectors)
end

function app.on_start(session)
    local player = get_player(session)
    app.view_sector(session, player, "Welcome to Trade Wars 2002!")
end

function app.view_sector(session, player, msg)
    local sectors = get_sectors()
    local sector = sectors[player.sector]

    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print("Sector: " .. player.sector .. "   Turns: " .. player.turns .. "   Credits: " .. player.credits)
    term.move_to(2, 7)
    term.set_color(11, 0)
    local warp_str = ""
    for i, w in ipairs(sector.warps) do
        warp_str = warp_str .. w .. " "
    end
    term.print("Warps: " .. warp_str)

    term.move_to(2, 8)
    if sector.port then
        term.set_color(10, 0)
        term.print("Port Class " .. sector.port.class .. " here.")
    else
        term.set_color(8, 0)
        term.print("No port here.")
    end

    term.move_to(2, 10)
    term.set_color(15, 0)
    term.print(msg or "")

    term.define_form(10)
    term.add_submit_button("nav", 2, 12)
    if sector.port then
        term.add_submit_button("port", 12, 12)
    end
    term.add_submit_button("status", 22, 12)
    term.add_submit_button("quit", 32, 12)

    term.flush_form()

    session.await_input(10, function(sub)
        if type(sub) == "string" then app.view_sector(session, player, "") return end
        local act = sub.submit

        if act == "nav" then
            app.nav_prompt(session, player)
        elseif act == "port" then
            app.port_menu(session, player)
        elseif act == "status" then
            app.view_status(session, player)
        elseif act == "quit" then
            save_player(session, player)
            session.load_app("main_menu")
        else
            app.view_sector(session, player, "Invalid action.")
        end
    end)
end

function app.nav_prompt(session, player)
    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 6)
    term.set_color(11, 0)
    term.print("Enter destination sector:")

    term.define_form(20)
    term.add_input_field("dest", 2, 8, 5, "")
    term.add_submit_button("warp", 2, 10)
    term.add_submit_button("cancel", 12, 10)
    term.flush_form()

    session.await_input(20, function(sub)
        if type(sub) == "string" then app.nav_prompt(session, player) return end

        if sub.submit == "cancel" then
            app.view_sector(session, player, "Navigation cancelled.")
            return
        end

        local dest = tonumber(sub.dest)
        if not dest then
            app.view_sector(session, player, "Invalid sector.")
            return
        end

        if player.turns <= 0 then
            app.view_sector(session, player, "You are out of turns!")
            return
        end

        local sectors = get_sectors()
        local valid_warp = false
        for _, w in ipairs(sectors[player.sector].warps) do
            if w == dest then valid_warp = true end
        end

        if valid_warp then
            player.sector = dest
            player.turns = player.turns - 1
            save_player(session, player)

            -- Encounter logic
            if math.random(1, 100) < 10 and dest ~= 1 then
                app.encounter(session, player, math.random(5, 20))
            else
                app.view_sector(session, player, "Warped to Sector " .. dest .. ".")
            end
        else
            app.view_sector(session, player, "No warp lane to Sector " .. dest .. ".")
        end
    end)
end

function app.encounter(session, player, f_fighters)
    -- Simple ferrengi encounter
    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 6)
    term.set_color(12, 0)
    term.print("WARNING: Ferrengi ambush!")

    term.move_to(2, 8)
    term.set_color(7, 0)
    term.print("Ferrengi Fighters: " .. f_fighters)
    term.move_to(2, 9)
    term.print("Your Fighters: " .. player.fighters)

    term.define_form(30)
    term.add_submit_button("fight", 2, 12)
    term.add_submit_button("run", 12, 12)
    term.flush_form()

    session.await_input(30, function(sub)
        if type(sub) == "string" then app.encounter(session, player, f_fighters) return end
        if sub.submit == "fight" then
            local p_loss = math.random(0, f_fighters)
            local f_loss = math.random(0, player.fighters)

            if p_loss > player.fighters then p_loss = player.fighters end
            if f_loss > f_fighters then f_loss = f_fighters end

            player.fighters = player.fighters - p_loss
            f_fighters = f_fighters - f_loss

            if f_fighters <= 0 then
                local bounty = math.random(100, 500)
                player.credits = player.credits + bounty
                save_player(session, player)
                app.view_sector(session, player, "You destroyed the Ferrengi! Bounty: " .. bounty .. " cr.")
            elseif player.fighters <= 0 then
                app.death(session, player, "You were destroyed by the Ferrengi.")
            else
                app.encounter(session, player, f_fighters) -- fight again
            end
        else
            if math.random(1, 100) > 50 then
                player.turns = player.turns - 1
                save_player(session, player)
                app.view_sector(session, player, "You narrowly escaped!")
            else
                local dmg = math.random(1, 5)
                player.fighters = player.fighters - dmg
                if player.fighters < 0 then player.fighters = 0 end
                if player.fighters == 0 then
                    app.death(session, player, "You were destroyed while fleeing.")
                else
                    app.encounter(session, player, f_fighters)
                end
            end
        end
    end)
end

function app.death(session, player, msg)
    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 6)
    term.set_color(12, 0)
    term.print("GAME OVER")
    term.move_to(2, 8)
    term.set_color(7, 0)
    term.print(msg)

    -- reset player
    db.set("tw_players", session.node_id(), nil)

    term.define_form(40)
    term.add_submit_button("continue", 2, 10)
    term.flush_form()

    session.await_input(40, function() session.load_app("main_menu") end)
end

function app.port_menu(session, player)
    local sectors = get_sectors()
    local port = sectors[player.sector].port
    if not port then
        app.view_sector(session, player, "No port here.")
        return
    end

    local p_rules = PORT_CLASSES[port.class]

    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 5)
    term.set_color(10, 0)
    term.print("Port Class " .. port.class)

    term.move_to(2, 7)
    term.set_color(7, 0)
    term.print("Item      Port Amt  Player Amt  Action")
    term.move_to(2, 8)
    term.print("----      --------  ----------  ------")

    local function item_line(y, name, p_amt, pl_amt, rule, price_base)
        term.move_to(2, y)
        local act = rule == 1 and "BUYING" or "SELLING"
        local price = rule == 1 and math.floor(price_base * 0.8) or math.floor(price_base * 1.2)
        term.print(string.format("%-8s  %-8d  %-10d  %s @ %d cr", name, p_amt, pl_amt, act, price))
        return act, price
    end

    local act_ore, pr_ore = item_line(9, "Fuel Ore", port.ore, player.ore, p_rules[1], 10)
    local act_org, pr_org = item_line(10, "Organics", port.org, player.org, p_rules[2], 20)
    local act_eqp, pr_eqp = item_line(11, "Equip", port.eqp, player.eqp, p_rules[3], 50)

    term.move_to(2, 13)
    term.print("Credits: " .. player.credits .. "   Holds Free: " .. (player.holds - player.ore - player.org - player.eqp))

    term.define_form(50)
    term.add_submit_button("trade_ore", 2, 15)
    term.add_submit_button("trade_org", 14, 15)
    term.add_submit_button("trade_eqp", 26, 15)
    term.add_submit_button("leave", 38, 15)
    term.flush_form()

    session.await_input(50, function(sub)
        if type(sub) == "string" then app.port_menu(session, player) return end
        local act = sub.submit

        if act == "leave" then
            app.view_sector(session, player, "You left the port.")
            return
        end

        local function do_trade(item_key, rule, price, port_amt, pl_amt)
            local holds_used = player.ore + player.org + player.eqp
            local holds_free = player.holds - holds_used

            if rule == 1 then -- Port is BUYING from player, player SELLS
                if pl_amt > 0 then
                    local qty = pl_amt
                    player.credits = player.credits + (qty * price)
                    player[item_key] = player[item_key] - qty
                    port[item_key] = port[item_key] + qty
                    save_player(session, player)
                    save_sectors(sectors)
                    app.port_menu(session, player)
                else
                    app.view_sector(session, player, "You have none of that to sell.")
                end
            else -- Port is SELLING to player, player BUYS
                if holds_free > 0 then
                    local max_buy = math.floor(player.credits / price)
                    local qty = math.min(holds_free, max_buy, port_amt)
                    if qty > 0 then
                        player.credits = player.credits - (qty * price)
                        player[item_key] = player[item_key] + qty
                        port[item_key] = port[item_key] - qty
                        save_player(session, player)
                        save_sectors(sectors)
                        app.port_menu(session, player)
                    else
                         app.view_sector(session, player, "Cannot buy.")
                    end
                else
                    app.view_sector(session, player, "Your holds are full.")
                end
            end
        end

        if act == "trade_ore" then
            do_trade("ore", p_rules[1], pr_ore, port.ore, player.ore)
        elseif act == "trade_org" then
            do_trade("org", p_rules[2], pr_org, port.org, player.org)
        elseif act == "trade_eqp" then
            do_trade("eqp", p_rules[3], pr_eqp, port.eqp, player.eqp)
        end
    end)
end

function app.view_status(session, player)
    term.clear()
    term.render_asset("tradewars_banner")
    term.move_to(2, 6)
    term.set_color(14, 0)
    term.print("--- PLAYER STATUS ---")
    term.move_to(2, 8)
    term.set_color(7, 0)
    term.print("Sector:  " .. player.sector)
    term.move_to(2, 9)
    term.print("Turns:   " .. player.turns)
    term.move_to(2, 10)
    term.print("Credits: " .. player.credits)
    term.move_to(2, 11)
    term.print("Fighters:" .. player.fighters)

    term.move_to(2, 13)
    term.print("Holds:   " .. (player.ore + player.org + player.eqp) .. " / " .. player.holds)
    term.move_to(2, 14)
    term.print("  Ore: " .. player.ore)
    term.move_to(2, 15)
    term.print("  Org: " .. player.org)
    term.move_to(2, 16)
    term.print("  Eqp: " .. player.eqp)

    term.define_form(60)
    term.add_submit_button("back", 2, 18)
    term.flush_form()

    session.await_input(60, function() app.view_sector(session, player, "") end)
end

return app
