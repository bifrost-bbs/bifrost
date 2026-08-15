-- Bifrost BBS Discussion Forum Application
local board = {}

function board.on_start(session)
    log.info("Discussion Boards application loaded.")
    term.clear()
    term.set_color(14, 0) -- Yellow on Black
    term.print("=== BIFROST DISCUSSION BOARDS ===\n\n")
    term.set_color(7, 0)
    term.print("Select a message to read:\n\n")
    
    term.define_form(2)
    
    term.add_submit_button("msg_1", 2, 5)
    term.print("    [General] Welcome to the Mesh! (by g8way)\n\n")

    term.add_submit_button("msg_2", 2, 7)
    term.print("    [Hardware] LilyGO T-Deck tips (by radio_fan)\n\n")

    term.add_submit_button("msg_3", 2, 9)
    term.print("    [Emergencies] Water station locations (by coordinator)\n\n")

    term.add_submit_button("main_menu", 2, 12)
    term.print("    Return to Main Menu\n")
    
    term.flush_form()
    
    session.await_input(2, function(submission)
        if type(submission) == "string" then
            board.on_start(session)
            return
        end

        local action = submission.submit
        log.info("User clicked message action: " .. tostring(action))
        if action == "main_menu" then
            session.load_app("00_main_menu")
        else
            term.clear()
            term.set_color(14, 0)
            term.print("=== MESSAGE VIEWER ===\n\n")
            term.set_color(7, 0)
            if action == "msg_1" then
                term.print("Subject: Welcome to the Mesh!\n")
                term.print("From: g8way\n\n")
                term.print("This is a decentralized mesh BBS running on Bifrost!\n")
            elseif action == "msg_2" then
                term.print("Subject: LilyGO T-Deck tips\n")
                term.print("From: radio_fan\n\n")
                term.print("Keep your Spreading Factor low to minimize airtime.\n")
            elseif action == "msg_3" then
                term.print("Subject: Water station locations\n")
                term.print("From: coordinator\n\n")
                term.print("Water stations are active at Sector 4 and 7.\n")
            end
            term.print("\n\n")
            
            term.define_form(3)
            term.add_submit_button("back", 2, 10)
            term.print("    Back to message list\n")
            term.flush_form()
            
            session.await_input(3, function() board.on_start(session) end)
        end
    end)
end

return board
