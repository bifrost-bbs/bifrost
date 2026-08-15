-- Bifrost BBS Discussion Forum Application
local board = {}

function board.on_start(session)
    log.info("Discussion Boards application loaded.")
    term.clear()
    term.set_color(14, 0) -- Yellow on Black
    term.print("=== BIFROST DISCUSSION BOARDS ===\n\n")
    term.set_color(7, 0)
    
    -- Stub messages list
    term.print("1. [General] Welcome to the Mesh! (by g8way)\n")
    term.print("2. [Hardware] LilyGO T-Deck tips (by radio_fan)\n")
    term.print("3. [Emergencies] Water station locations (by coordinator)\n\n")
    
    term.print("[M] Main Menu\n")
    term.print("Select message index to read: ")
    term.flush()
    
    session.await_input(1, function(choice)
        log.info("User typed board choice: " .. choice)
        if choice:upper() == "M" then
            log.info("Returning to main menu.")
            session.load_app("00_main_menu")
        else
            log.warn("Invalid message index or action requested: " .. choice)
            term.clear()
            term.print("Message reading not implemented in this preview.\n")
            term.print("Press any key to return...\n")
            term.flush()
            session.await_input(1, function() board.on_start(session) end)
        end
    end)
end

return board
