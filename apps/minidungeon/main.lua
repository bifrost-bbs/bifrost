local app = {}

local MONSTER_TYPES = {
    -- Tier 1 (Level 1+)
    { name = "Mesh Goblin", min_level = 1, hp_base = 10, str_base = 6, dex_base = 8, xp_base = 15 },
    { name = "Bit Beetle", min_level = 1, hp_base = 8, str_base = 5, dex_base = 9, xp_base = 12 },
    { name = "Antenna Mimic", min_level = 1, hp_base = 12, str_base = 7, dex_base = 10, xp_base = 20 },
    -- Tier 2 (Level 2+)
    { name = "LoRa Orc", min_level = 2, hp_base = 18, str_base = 10, dex_base = 8, xp_base = 25 },
    { name = "Packet Loss Wraith", min_level = 2, hp_base = 15, str_base = 8, dex_base = 12, xp_base = 30 },
    -- Tier 3 (Level 3+)
    { name = "Bytecode Behemoth", min_level = 3, hp_base = 28, str_base = 13, dex_base = 6, xp_base = 50 },
    { name = "Firmware Dragon", min_level = 4, hp_base = 40, str_base = 16, dex_base = 10, xp_base = 75 }
}

-- Rebalanced event pool: positive/NPC encounters are now rare (~1/4 of previous occurrence)
local EVENT_TYPES = {
    "EMPTY", "EMPTY", "EMPTY", "EMPTY", "EMPTY", "EMPTY", "EMPTY", "EMPTY",
    "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER", "MONSTER",
    "TRAP", "TRAP", "TRAP",
    "TREASURE",
    "FOUNTAIN",
    "NPC",
    "EASTER_EGG"
}

-- Exponential XP progression: 50 for Lvl 2, 150 total for Lvl 3, 350 total for Lvl 4, etc.
local function xp_needed(level)
    return 50 * (math.floor(2 ^ level) - 1)
end

local function rest_cost(player)
    local cost = 10 - math.floor((player.wis or 10) / 4)
    if cost < 5 then cost = 5 end
    return cost
end

local function generate_map(dungeon_level)
    local map = {}
    for y = 1, 5 do
        map[y] = {}
        for x = 1, 5 do
            map[y][x] = {
                type = EVENT_TYPES[math.random(1, #EVENT_TYPES)],
                cleared = false
            }
        end
    end
    local start_x, start_y = math.random(1, 5), math.random(1, 5)
    local exit_x, exit_y
    repeat
        exit_x, exit_y = math.random(1, 5), math.random(1, 5)
    until exit_x ~= start_x or exit_y ~= start_y

    map[start_y][start_x].type = "ENTRANCE"
    map[start_y][start_x].cleared = true
    map[exit_y][exit_x].type = "EXIT"

    return map, start_x, start_y
end

local function init_player(session)
    local map, sx, sy = generate_map(1)
    return {
        hp = 25,
        max_hp = 25,
        str = 10,
        dex = 10,
        con = 10,
        int = 10,
        wis = 10,
        cha = 10,
        xp = 0,
        level = 1,
        gold = 0,
        potions = 2,
        dungeon_level = 1,
        x = sx,
        y = sy,
        map = map
    }
end

local function save_player(session, player)
    db.set("dungeon_players", session.node_id(), player)
end

function app.on_start(session)
    local player = db.get("dungeon_players", session.node_id())
    if not player or type(player) ~= "table" or not player.hp or player.hp <= 0 then
        player = init_player(session)
        save_player(session, player)
    end
    app.view_room(session, player, "Welcome to the LoRa Catacombs, Level " .. player.dungeon_level .. "!")
end

function app.view_room(session, player, msg)
    term.clear()
    term.render_asset("dungeon_banner")
    term.move_to(2, 6)

    term.render_template("dungeon_hud", {
        tostring(player.dungeon_level),
        tostring(player.level),
        session.callsign() or "Adventurer",
        tostring(player.hp),
        tostring(player.max_hp),
        tostring(player.gold),
        tostring(player.xp),
        tostring(xp_needed(player.level)),
        tostring(player.potions),
        tostring(player.str),
        tostring(player.dex),
        tostring(player.con),
        tostring(player.int),
        tostring(player.wis),
        tostring(player.cha)
    })

    term.move_to(2, 10)
    term.set_color(11, 0)
    term.print(msg or "")

    -- Draw minimap
    local map_start_x = 42
    local map_start_y = 6
    term.set_color(8, 0)
    for y = 1, 5 do
        term.move_to(map_start_x, map_start_y + y - 1)
        local row_str = ""
        for x = 1, 5 do
            if player.x == x and player.y == y then
                row_str = row_str .. "[@]"
            else
                local rm = player.map[y][x]
                if not rm.cleared then
                    row_str = row_str .. "[?]"
                elseif rm.type == "ENTRANCE" then
                    row_str = row_str .. "[E]"
                elseif rm.type == "EXIT" then
                    row_str = row_str .. "[X]"
                else
                    row_str = row_str .. "[.]"
                end
            end
        end
        term.print(row_str)
    end

    local current_room = player.map[player.y][player.x]
    term.render_menu("dungeon_menu", {
        north = (player.y > 1),
        south = (player.y < 5),
        east = (player.x < 5),
        west = (player.x > 1),
        potion = (player.potions > 0 and player.hp < player.max_hp),
        rest = true,
        descend = (current_room.type == "EXIT"),
        exit = true
    })
    term.flush_form()

    session.await_input(10, function(sub)
        if type(sub) == "string" then app.view_room(session, player, "") return end
        local act = sub.submit

        if act == "north" then player.y = player.y - 1; app.do_event(session, player)
        elseif act == "south" then player.y = player.y + 1; app.do_event(session, player)
        elseif act == "east" then player.x = player.x + 1; app.do_event(session, player)
        elseif act == "west" then player.x = player.x - 1; app.do_event(session, player)
        elseif act == "descend" then app.descend(session, player)
        elseif act == "potion" then
            if player.potions > 0 then
                player.potions = player.potions - 1
                player.hp = math.min(player.max_hp, player.hp + 15)
                save_player(session, player)
                app.view_room(session, player, "Gulp! Restored 15 HP.")
            else
                app.view_room(session, player, "No potions left!")
            end
        elseif act == "rest" then
            local cost = rest_cost(player)
            if player.gold >= cost then
                player.gold = player.gold - cost
                player.hp = player.max_hp
                save_player(session, player)
                app.view_room(session, player, "You rested for " .. cost .. " Gold. HP fully restored!")
            else
                app.view_room(session, player, "Not enough Gold to rest! Need " .. cost .. "G.")
            end
        elseif act == "exit" or act == "quit" then
            save_player(session, player)
            session.load_app("main_menu")
        else
            app.view_room(session, player, "Invalid action.")
        end
    end)
end

function app.do_event(session, player)
    local room = player.map[player.y][player.x]
    if room.cleared then
        save_player(session, player)
        app.view_room(session, player, "You enter a cleared room.")
        return
    end

    room.cleared = true
    save_player(session, player)
    
    local rtype = room.type
    if rtype == "EMPTY" then
        app.view_room(session, player, "The room is empty. Dust settles softly.")
    elseif rtype == "TREASURE" then
        local base_amt = math.random(5, 15) + (player.dungeon_level * 4)
        local cha_bonus = math.floor(player.cha * 1.2)
        local amt = base_amt + cha_bonus
        player.gold = player.gold + amt
        local has_pot = math.random(1, 100) > (85 - math.floor(player.wis * 1.5))
        local msg = "You found a chest with " .. amt .. " Gold!"
        if has_pot then
            player.potions = player.potions + 1
            msg = msg .. " And a potion!"
        end
        save_player(session, player)
        app.msg_view(session, player, 20, msg, 10) -- green
    elseif rtype == "TRAP" then
        -- Wisdom to spot/disarm, Dexterity to dodge, Constitution to mitigate damage
        local spot_chance = math.min(60, player.wis * 3)
        if math.random(1, 100) <= spot_chance then
            player.xp = player.xp + 5
            save_player(session, player)
            app.msg_view(session, player, 21, "Your keen Wisdom spotted a trap! You safely disarmed it (+5 XP).", 10)
            return
        end
        local dodge_chance = math.min(60, player.dex * 3)
        if math.random(1, 100) <= dodge_chance then
            app.msg_view(session, player, 21, "A trap sprang, but your quick Dexterity allowed you to leap clear!", 11)
            return
        end
        local base_dmg = math.random(3, 7) + player.dungeon_level
        local con_mitigation = math.floor(player.con / 4)
        local dmg = math.max(1, base_dmg - con_mitigation)
        player.hp = player.hp - dmg
        save_player(session, player)
        if player.hp <= 0 then
            app.game_over(session, player, "You stepped on a deadly trap and perished!")
        else
            app.msg_view(session, player, 21, "You triggered a trap and took " .. dmg .. " damage!", 12)
        end
    elseif rtype == "FOUNTAIN" then
        -- Wisdom boosts fountain healing
        local heal = math.random(6, 12) + math.floor(player.wis * 1.2)
        player.hp = math.min(player.max_hp, player.hp + heal)
        save_player(session, player)
        app.msg_view(session, player, 22, "You drink from a glowing fountain and restore " .. heal .. " HP.", 11)
    elseif rtype == "NPC" then
        -- Charisma enhances NPC interactions and gifts
        local msgs = {
            "A wandering merchant is charmed and gives you a potion!",
            "An old wizard casts a healing aura on you.",
            "A friendly mesh nomad offers you some gold."
        }
        local choice = math.random(1, #msgs)
        if choice == 1 then
            player.potions = player.potions + 1
        elseif choice == 2 then
            local heal = 10 + math.floor(player.wis * 0.8)
            player.hp = math.min(player.max_hp, player.hp + heal)
        elseif choice == 3 then
            local gold_amt = 10 + math.floor(player.cha * 1.5)
            player.gold = player.gold + gold_amt
            msgs[3] = "A friendly mesh nomad offers you " .. gold_amt .. " gold."
        end
        if player.cha >= 14 and math.random(1, 100) <= 50 then
            player.potions = player.potions + 1
            msgs[choice] = msgs[choice] .. " (Bonus potion thanks to high Charisma!)"
        end
        save_player(session, player)
        app.msg_view(session, player, 23, msgs[choice], 14)
    elseif rtype == "EASTER_EGG" then
        local eggs = {
            "You found a teapot. It whispers: '418 I'm a teapot'.",
            "It is pitch black. You are likely to be eaten by a grue.",
            "There's a note: 'Try finger, but hole.'",
            "A terminal flashes: 'Guru Meditation #00000004.0000AAC0'."
        }
        app.msg_view(session, player, 24, eggs[math.random(1, #eggs)], 13)
    elseif rtype == "EXIT" then
        app.view_room(session, player, "You found the stairs leading down!")
    elseif rtype == "MONSTER" then
        local available_monsters = {}
        for _, m in ipairs(MONSTER_TYPES) do
            if m.min_level <= player.dungeon_level then
                table.insert(available_monsters, m)
            end
        end
        if #available_monsters == 0 then
            available_monsters = { MONSTER_TYPES[1] }
        end
        local m_base = available_monsters[math.random(1, #available_monsters)]
        local depth_bonus = player.dungeon_level - 1
        local m_hp = m_base.hp_base + (depth_bonus * 3) + math.random(-1, 2)
        if m_hp < 5 then m_hp = 5 end
        local monster = {
            name = m_base.name,
            hp = m_hp,
            max_hp = m_hp,
            str = m_base.str_base + depth_bonus,
            dex = m_base.dex_base + depth_bonus,
            xp = m_base.xp_base + (player.dungeon_level * 5)
        }
        app.battle_round(session, player, monster, "A wild " .. monster.name .. " attacks!")
    else
        app.view_room(session, player, "You enter a strange room...")
    end
end

function app.msg_view(session, player, form_id, msg, color)
    term.clear()
    term.render_asset("dungeon_banner")
    term.move_to(2, 10)
    term.set_color(color or 7, 0)
    term.print(msg)
    term.define_form(form_id)
    term.add_submit_button("continue", 2, 13)
    term.flush_form()
    session.await_input(form_id, function() app.view_room(session, player, "You continue exploring.") end)
end

function app.battle_round(session, player, monster, msg)
    term.clear()
    term.render_asset("dungeon_banner")
    term.move_to(2, 6)
    term.set_color(12, 0)
    term.print("=== BATTLE ===")

    term.move_to(2, 8)
    term.set_color(14, 0)
    term.print(string.format("%s (Lvl %d)", session.callsign(), player.level))
    term.move_to(2, 9)
    term.set_color(7, 0)
    term.print(string.format("HP: %d/%d | Pots: %d", player.hp, player.max_hp, player.potions))
    term.move_to(2, 10)
    term.set_color(10, 0)
    term.print(string.format("STR:%d DEX:%d CON:%d INT:%d WIS:%d CHA:%d", player.str, player.dex, player.con, player.int, player.wis, player.cha))

    term.move_to(35, 8)
    term.set_color(13, 0)
    term.print(monster.name)
    term.move_to(35, 9)
    term.set_color(7, 0)
    term.print(string.format("HP: %d/%d", monster.hp, monster.max_hp))

    term.move_to(2, 12)
    term.set_color(15, 0)
    term.print(msg or "")

    term.define_form(30)
    term.add_submit_button("attack", 2, 14)
    term.add_submit_button("cast", 10, 14)
    term.add_submit_button("potion", 18, 14)
    term.add_submit_button("flee", 26, 14)
    term.flush_form()

    session.await_input(30, function(sub)
        if type(sub) == "string" then app.battle_round(session, player, monster, "") return end
        local act = sub.submit

        if act == "attack" then
            -- Player physical attack (STR damage, DEX accuracy, INT crit chance)
            local hit_chance = 50 + (player.dex * 2) - (monster.dex * 2)
            if hit_chance < 15 then hit_chance = 15 end
            if hit_chance > 95 then hit_chance = 95 end
            
            local result_msg = ""
            if math.random(1, 100) <= hit_chance then
                local base_dmg = math.floor(player.str / 2) + math.random(1, 4)
                local is_crit = math.random(1, 100) <= math.floor(player.int * 1.5)
                local dmg = base_dmg
                if is_crit then
                    dmg = math.floor(base_dmg * 1.5) + 1
                    result_msg = "CRITICAL HIT! Hit " .. monster.name .. " for " .. dmg .. " dmg! "
                else
                    result_msg = "You hit " .. monster.name .. " for " .. dmg .. " dmg! "
                end
                monster.hp = monster.hp - dmg
            else
                result_msg = "You missed! "
            end
            
            if monster.hp <= 0 then
                app.victory(session, player, monster)
                return
            end
            
            -- Monster attacks (CON mitigation, DEX evasion)
            local m_hit_chance = 50 + (monster.dex * 2) - (player.dex * 2)
            if m_hit_chance < 15 then m_hit_chance = 15 end
            if m_hit_chance > 95 then m_hit_chance = 95 end

            if math.random(1, 100) <= m_hit_chance then
                local mdmg = math.floor(monster.str / 2) + math.random(1, 4) - math.floor(player.con / 4)
                if mdmg < 1 then mdmg = 1 end
                player.hp = player.hp - mdmg
                result_msg = result_msg .. monster.name .. " hits you for " .. mdmg .. " dmg!"
            else
                result_msg = result_msg .. monster.name .. " misses you!"
            end

            save_player(session, player)
            if player.hp <= 0 then
                app.game_over(session, player, "You were slain by a " .. monster.name .. "!")
            else
                app.battle_round(session, player, monster, result_msg)
            end

        elseif act == "cast" then
            -- Magic spell (INT damage, never misses)
            local spell_dmg = math.floor(player.int * 0.7) + math.random(2, 5)
            monster.hp = monster.hp - spell_dmg
            local result_msg = "You cast Arcane Bolt for " .. spell_dmg .. " magic dmg! "

            if monster.hp <= 0 then
                app.victory(session, player, monster)
                return
            end

            -- Monster attacks
            local m_hit_chance = 50 + (monster.dex * 2) - (player.dex * 2)
            if m_hit_chance < 15 then m_hit_chance = 15 end
            if m_hit_chance > 95 then m_hit_chance = 95 end

            if math.random(1, 100) <= m_hit_chance then
                local mdmg = math.floor(monster.str / 2) + math.random(1, 4) - math.floor(player.con / 4)
                if mdmg < 1 then mdmg = 1 end
                player.hp = player.hp - mdmg
                result_msg = result_msg .. monster.name .. " hits you for " .. mdmg .. " dmg!"
            else
                result_msg = result_msg .. monster.name .. " misses you!"
            end

            save_player(session, player)
            if player.hp <= 0 then
                app.game_over(session, player, "You were slain by a " .. monster.name .. "!")
            else
                app.battle_round(session, player, monster, result_msg)
            end

        elseif act == "potion" then
            if player.potions > 0 then
                player.potions = player.potions - 1
                local heal = 20 + math.floor(player.max_hp * 0.2) + math.floor(player.con / 2)
                player.hp = math.min(player.max_hp, player.hp + heal)
                save_player(session, player)
                app.battle_round(session, player, monster, "You drank a potion and restored " .. heal .. " HP.")
            else
                app.battle_round(session, player, monster, "You don't have any potions!")
            end
        elseif act == "flee" then
            local flee_chance = 45 + (player.dex * 2) + math.floor(player.cha * 1.2) - (monster.dex * 2)
            if flee_chance < 15 then flee_chance = 15 end
            if flee_chance > 90 then flee_chance = 90 end

            if math.random(1, 100) <= flee_chance then
                app.view_room(session, player, "You successfully fled the battle!")
            else
                local mdmg = math.floor(monster.str / 2) + math.random(1, 4) - math.floor(player.con / 4)
                if mdmg < 1 then mdmg = 1 end
                player.hp = player.hp - mdmg
                save_player(session, player)
                if player.hp <= 0 then
                    app.game_over(session, player, "You failed to flee and were struck down!")
                else
                    app.battle_round(session, player, monster, "Failed to flee! Took " .. mdmg .. " dmg!")
                end
            end
        end
    end)
end

function app.victory(session, player, monster)
    -- INT provides bonus XP from encounters
    local xp_gain = monster.xp + math.floor(player.int * 0.5)
    player.xp = player.xp + xp_gain
    -- CHA provides bonus gold from defeated monsters
    local gold_gained = math.random(2, 6) + (player.dungeon_level * 2) + math.floor(player.cha * 0.5)
    player.gold = player.gold + gold_gained

    local msg = "Victory! Gained " .. xp_gain .. " XP and " .. gold_gained .. " Gold!"
    save_player(session, player)

    if player.xp >= xp_needed(player.level) then
        player.level = player.level + 1
        app.level_up(session, player)
    else
        app.msg_view(session, player, 31, msg, 10)
    end
end

function app.level_up(session, player)
    player.max_hp = player.max_hp + 5 + math.floor(player.con / 2)
    player.hp = player.max_hp
    save_player(session, player)

    term.clear()
    term.render_asset("dungeon_banner")
    term.move_to(2, 8)
    term.set_color(14, 0)
    term.print("LEVEL UP! You are now level " .. player.level .. "!")
    term.move_to(2, 9)
    term.set_color(7, 0)
    term.print("Choose a stat to increase:")

    term.define_form(40)
    term.add_submit_button("str", 2, 11)
    term.add_submit_button("dex", 10, 11)
    term.add_submit_button("con", 18, 11)
    term.add_submit_button("int", 26, 11)
    term.add_submit_button("wis", 34, 11)
    term.add_submit_button("cha", 42, 11)
    term.flush_form()
    
    session.await_input(40, function(sub)
        if type(sub) == "string" then app.level_up(session, player) return end
        local act = sub.submit
        if act == "str" then player.str = player.str + 1
        elseif act == "dex" then player.dex = player.dex + 1
        elseif act == "con" then player.con = player.con + 1
        elseif act == "int" then player.int = player.int + 1
        elseif act == "wis" then player.wis = player.wis + 1
        elseif act == "cha" then player.cha = player.cha + 1
        else player.str = player.str + 1 end

        save_player(session, player)
        app.view_room(session, player, "You feel stronger! (" .. string.upper(act) .. " increased)")
    end)
end

function app.descend(session, player)
    player.dungeon_level = player.dungeon_level + 1
    local map, sx, sy = generate_map(player.dungeon_level)
    player.map = map
    player.x = sx
    player.y = sy
    save_player(session, player)

    app.msg_view(session, player, 50, "You descend deeper into the darkness... Welcome to Level " .. player.dungeon_level .. "!", 13)
end

function app.game_over(session, player, cause)
    db.set("dungeon_players", session.node_id(), nil) -- Clear save
    term.clear()
    term.render_asset("dungeon_banner")
    term.move_to(2, 8)
    term.set_color(12, 0)
    term.print("=== GAME OVER ===")
    term.move_to(2, 10)
    term.set_color(7, 0)
    term.print(cause)
    term.move_to(2, 11)
    term.print(string.format("You reached Dungeon Level %d and Character Level %d.", player.dungeon_level, player.level))
    
    term.define_form(60)
    term.add_submit_button("exit", 2, 14)
    term.flush_form()
    
    session.await_input(60, function() session.load_app("main_menu") end)
end

return app
