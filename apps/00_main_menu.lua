-- Bifrost BBS Main Menu Application using Declarative Forms
local menu = {}

function menu.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    term.set_color(11, 0) -- Cyan on Black
    term.print("========================================\n")
    term.print("       BIFROST BBS FORM MAIN MENU       \n")
    term.print("========================================\n")
    term.set_color(7, 0) -- White on Black

    if not user then
        -- Force register nickname on very first connection
        term.print(" Welcome to Bifrost! Please set a nickname:\n\n")
        term.define_form(1)
        term.print("  Your Nickname: ")
        term.add_input_field("nickname", 17, 5, 15, "Operator")
        term.print("\n\n")
        term.add_submit_button("register", 2, 8)
        term.flush_form()

        session.await_input(1, function(submission)
            if type(submission) == "string" then
                menu.on_start(session)
                return
            end
            local nick = submission.nickname or "Operator"
            db.set("users", user_id, { nickname = nick })
            log.info("New user registered nickname: " .. nick)
            menu.on_start(session)
        end)
    else
        -- Hello [nickname]
        term.print("  Hello, " .. user.nickname .. "!\n\n")
        term.print("Select options using Tab/Arrows and Enter:\n\n")

        term.define_form(10)
        term.add_submit_button("read_boards", 2, 8)
        term.add_submit_button("door_game", 18, 8)
        term.add_submit_button("profile", 34, 8)
        term.add_submit_button("marketplace", 2, 9)
        
        -- Show Admin Panel only if the user has admin permission
        local is_admin = session.has_permission("admin")
        log.info("Session admin check: " .. tostring(is_admin))
        if is_admin then
            term.add_submit_button("admin", 2, 11)
            term.add_submit_button("logout", 18, 11)
        else
            term.add_submit_button("logout", 2, 11)
        end
        term.flush_form()

        session.await_input(10, function(submission)
            if type(submission) == "string" then
                menu.on_start(session)
                return
            end

            local action = submission.submit
            log.info("Main menu selected action: " .. tostring(action))

            if action == "read_boards" then
                session.load_app("10_messages")
            elseif action == "marketplace" then
                session.load_app("50_marketplace")
            elseif action == "door_game" then
                session.load_app("30_doorgames/minidungeon")
            elseif action == "profile" then
                session.load_app("20_profile")
            elseif action == "admin" then
                if session.has_permission("admin") then
                    session.load_app("40_admin")
                else
                    log.warn("Access denied: User " .. user.nickname .. " requested admin app without permission.")
                    menu.on_start(session)
                end
            elseif action == "logout" then
                log.info("User logged out: " .. user.nickname)
                term.clear()
                term.print("Goodbye, " .. user.nickname .. "!\n")
                term.flush()
                session.close()
            else
                menu.on_start(session)
            end
        end)
    end
end

return menu
