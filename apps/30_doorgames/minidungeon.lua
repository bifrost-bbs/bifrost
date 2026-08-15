-- Simple Turn-Based Combat Door Game for MeshBBS
local app = {}

function app.on_start(session)
    term.clear()
    -- Renders public cached banner (0 airtime if pre-cached via broadcast)
    term.render_asset("ASSET_DUNGEON_BANNER")
    term.move_to(2, 8)
    term.set_color(14, 0) -- Yellow on Black
    term.print("=== THE LORA CATACOMBS ===\n")
    
    local player = db.get("dungeon_players", session.node_id()) or { hp = 20, gold = 0, level = 1 }
    term.move_to(2, 10)
    term.set_color(7, 0)
    term.print(string.format("Hero: %s | HP: %d/20 | Gold: %d\n", session.callsign(), player.hp, player.gold))
    
    term.move_to(2, 12)
    term.print("[1] Explore Crypt  [2] Rest at Camp  [Q] Exit\n")
    term.print("Enter choice: ")
    term.flush()
    
    session.await_input(1, function(input)
        if input == "1" then
            app.battle(session, player)
        elseif input == "2" then
            player.hp = 20
            db.set("dungeon_players", session.node_id(), player)
            term.move_to(2, 14)
            term.set_color(10, 0) -- Green
            term.print("You rested and restored your HP! Press any key to continue...\n")
            term.flush()
            session.await_input(1, function() app.on_start(session) end)
        else
            session.load_app("00_main_menu")
        end
    end)
end

function app.battle(session, player)
    local monster_hp = math.random(5, 12)
    term.move_to(2, 14)
    term.set_color(12, 0) -- Light Red
    term.print(string.format("A Wild Mesh Goblin appears! (HP: %d)\n", monster_hp))
    player.gold = player.gold + 5
    player.hp = math.max(1, player.hp - 3)
    db.set("dungeon_players", session.node_id(), player)
    term.move_to(2, 16)
    term.set_color(11, 0) -- Cyan
    term.print("You defeated the Goblin and earned 5 Gold!\n")
    term.print("Press any key to return...\n")
    term.flush()
    session.await_input(1, function() app.on_start(session) end)
end

return app
