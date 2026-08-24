-- Bifrost BBS Profile Manager Application
local profile = {}

function profile.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id) or { nickname = "Operator", bio = "A quiet operator." }
    local current_bio = user.bio or "A quiet operator."

    term.clear()
    term.set_color(13, 0) -- Magenta on Black
    term.print("=== PROFILE MANAGER ===\n\n")
    term.set_color(7, 0)

    term.define_form(20)
    term.print("  Nickname: ")
    term.add_input_field("nickname", 12, 2, 15, user.nickname)
    term.print("\n\n")
    
    term.print("  Bio (Wrap-enabled block):\n")
    term.add_multiline_field("bio", 2, 6, 36, 4, current_bio)
    term.print("\n\n\n\n\n")
    
    term.add_submit_button("save", 2, 12)
    term.add_submit_button("cancel", 12, 12)
    term.flush_form()

    session.await_input(20, function(submission)
        if type(submission) == "string" then
            local s = submission:lower()
            if s == "q" or s == "b" or s == "m" or s == "cancel" or s == "exit" or s == "quit" then
                session.load_app("main_menu")
            else
                profile.on_start(session)
            end
            return
        end

        local action = submission.submit
        log.info("Profile action: " .. tostring(action))

        if action == "save" then
            local new_nick = submission.nickname or user.nickname
            local new_bio = submission.bio or current_bio
            local updated_user = user or {}
            updated_user.nickname = new_nick
            updated_user.bio = new_bio
            db.set("users", user_id, updated_user)
            log.info("Profile updated: nickname=" .. new_nick .. ", bio=" .. new_bio)
            session.load_app("main_menu")
        else
            log.info("Profile edit canceled.")
            session.load_app("main_menu")
        end
    end)
end

function profile.on_resume(session)
    profile.on_start(session)
end

return profile
