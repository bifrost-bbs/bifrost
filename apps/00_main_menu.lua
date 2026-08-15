-- Bifrost BBS Main Menu Application using Declarative Forms
local menu = {}

function menu.on_start(session)
    log.info("Main menu started for user session: " .. session.node_id())
    term.clear()
    term.set_color(11, 0) -- Cyan on Black
    term.print("========================================\n")
    term.print("       BIFROST BBS FORM MAIN MENU       \n")
    term.print("========================================\n")
    term.set_color(7, 0) -- White on Black
    term.print("Select options using Tab and Enter:\n\n")
    
    term.define_form(1)
    term.print("  Your Nickname: ")
    term.add_input_field("nickname", 17, 5, 15, "Operator")
    term.print("\n\n")
    term.print("  Actions:\n")
    term.add_submit_button("read_boards", 2, 8)
    term.add_submit_button("door_game", 18, 8)
    term.add_submit_button("logout", 32, 8)
    term.flush_form()

    session.await_input(1, function(submission)
        -- If submission is a string (fallback key input), ignore or re-render
        if type(submission) == "string" then
            log.warn("Received string input fallback instead of form submission: " .. submission)
            menu.on_start(session)
            return
        end

        local nick = submission.nickname or "Operator"
        log.info("User set nickname to: " .. nick)
        db.set("users", session.node_id(), { nickname = nick })

        log.info("Form submitted with button action: " .. tostring(submission.submit))
        if submission.submit == "read_boards" then
            session.load_app("10_messages")
        elseif submission.submit == "door_game" then
            session.load_app("30_doorgames/minidungeon")
        elseif submission.submit == "logout" then
            log.info("User requested logout. Closing session.")
            term.clear()
            term.print("Goodbye, " .. nick .. "!\n")
            term.flush()
            session.close()
        else
            menu.on_start(session)
        end
    end)
end

return menu
