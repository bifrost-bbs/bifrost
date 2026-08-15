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
    
    term.define_form(4)
    term.add_submit_button("explore", 2, 12)
    term.add_submit_button("rest", 18, 12)
    term.add_submit_button("exit", 32, 12)
    term.flush_form()
    
    session.await_input(4, function(submission)
        if type(submission) == "string" then
            app.on_start(session)
            return
        end

        local action = submission.submit
        log.info("Dungeon game option selected: " .. tostring(action))
        if action == "explore" then
            app.battle(session, player)
        elseif action == "rest" then
            player.hp = 20
            db.set("dungeon_players", session.node_id(), player)
            
            term.clear()
            term.render_asset("ASSET_DUNGEON_BANNER")
            term.move_to(2, 8)
            term.set_color(10, 0) -- Green
            term.print("You rested and restored your HP!\n\n")
            
            term.define_form(5)
            term.add_submit_button("continue", 2, 11)
            term.flush_form()
            
            session.await_input(5, function() app.on_start(session) end)
        else
            session.load_app("00_main_menu")
        end
    end)
end

function app.battle(session, player)
    local monster_hp = math.random(5, 12)
    term.clear()
    term.render_asset("ASSET_DUNGEON_BANNER")
    term.move_to(2, 8)
    term.set_color(12, 0) -- Light Red
    term.print(string.format("A Wild Mesh Goblin appears! (HP: %d)\n", monster_hp))
    player.gold = player.gold + 5
    player.hp = math.max(1, player.hp - 3)
    db.set("dungeon_players", session.node_id(), player)
    
    term.move_to(2, 10)
    term.set_color(11, 0) -- Cyan
    term.print("You defeated the Goblin and earned 5 Gold!\n\n")
    
    term.define_form(6)
    term.add_submit_button("continue", 2, 13)
    term.flush_form()
    
    session.await_input(6, function() app.on_start(session) end)
end

return app
