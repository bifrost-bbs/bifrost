-- Bifrost BBS Admin Panel Application
local admin = {}

function admin.on_start(session)
    if not session.has_permission("admin") then
        log.warn("Unauthorized access attempt to admin app.")
        session.load_app("00_main_menu")
        return
    end

    term.clear()
    term.set_color(12, 0) -- Light Red on Black
    term.print("=== ADMIN PANEL ===\n\n")
    term.set_color(7, 0)

    term.print("Registered Nodes list:\n")
    local user_keys = db.keys("users")
    local y_offset = 5
    
    for i, user_node_id in ipairs(user_keys) do
        local user_data = db.get("users", user_node_id) or { nickname = "Unknown" }
        local perms = db.get("permissions", user_node_id) or {}
        local has_admin = false
        for _, p in ipairs(perms) do
            if p == "admin" then has_admin = true end
        end
        local admin_str = has_admin and "[ADMIN]" or "[USER]"
        term.print(string.format("  %d. %s (%s) %s\n", i, user_data.nickname, user_node_id:sub(1, 8), admin_str))
        y_offset = y_offset + 1
    end

    term.print("\n Enter target user node hex prefix:\n")
    term.define_form(40)
    term.add_input_field("target_id", 2, y_offset + 2, 8, "")
    
    term.add_submit_button("toggle_admin", 12, y_offset + 2)
    term.add_submit_button("back", 28, y_offset + 2)
    term.flush_form()

    session.await_input(40, function(submission)
        if type(submission) == "string" then
            admin.on_start(session)
            return
        end

        local action = submission.submit
        if action == "back" then
            session.load_app("00_main_menu")
        elseif action == "toggle_admin" then
            local prefix = submission.target_id or ""
            if prefix ~= "" then
                -- Match prefix against user keys
                local matched_id = nil
                for _, user_node_id in ipairs(user_keys) do
                    if user_node_id:sub(1, #prefix) == prefix then
                        matched_id = user_node_id
                        break
                    end
                end

                if matched_id then
                    local perms = db.get("permissions", matched_id) or { "read", "write" }
                    local index = nil
                    for idx, p in ipairs(perms) do
                        if p == "admin" then index = idx break end
                    end
                    if index then
                        table.remove(perms, index)
                    else
                        table.insert(perms, "admin")
                    end
                    db.set("permissions", matched_id, perms)
                    log.info("Admin toggled permissions for " .. matched_id)
                else
                    log.warn("Admin target prefix not found: " .. prefix)
                end
            end
            admin.on_start(session)
        else
            admin.on_start(session)
        end
    end)
end

return admin
