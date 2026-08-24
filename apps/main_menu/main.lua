-- Bifrost BBS Config-Driven Dynamic Main Menu
local menu = {}

function menu.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    local cfg = nil
    if type(session.get_menu_config) == "function" then
        cfg = session.get_menu_config()
    end
    if not cfg then
        cfg = {
            banner_asset = "main_menu_banner",
            title = "=== BIFROST MESHBBS ===",
            header_fg = 14,
            header_bg = 0,
            layout = "grid",
            start_col = 2,
            start_row = 10,
            col_width = 16,
            show_logout = true
        }
    end

    if cfg.banner_asset and cfg.banner_asset ~= "" then
        term.render_asset(cfg.banner_asset)
    end

    term.move_to(2, 6)
    term.set_color(cfg.header_fg or 14, cfg.header_bg or 0)

    if not user or not user.nickname then
        local default_nick = "Operator"
        if user and user.node_name then
            default_nick = user.node_name
        end

        -- Force register nickname on very first connection
        term.print("Welcome to Bifrost! Please set a nickname:\n")
        term.define_form(1)
        term.print("  Your Nickname: ")
        term.add_input_field("nickname", 18, 8, 15, default_nick)
        term.print("\n")
        term.add_submit_button("register", 2, 10)
        term.flush_form()

        session.await_input(1, function(submission)
            if type(submission) == "string" then
                menu.on_start(session)
                return
            end
            local nick = submission.nickname or default_nick
            local updated_user = user or {}
            updated_user.nickname = nick
            db.set("users", user_id, updated_user)
            log.info("New user registered nickname: " .. nick)
            menu.on_start(session)
        end)
        return
    end

    term.print("Hello, " .. user.nickname .. "!\n")
    term.set_color(7, 0)
    term.print("Select options using Tab/Arrows or Hotkeys:\n\n")

    local is_admin = session.has_permission("admin")
    local apps = nil
    if type(session.get_apps) == "function" then
        apps = session.get_apps()
    end
    if not apps or #apps == 0 then
        apps = {
            { id = "messages", name = "Message Boards", admin_only = false },
            { id = "minidungeon", name = "Mini Dungeon", admin_only = false },
            { id = "voidtrader", name = "Void Trader", admin_only = false },
            { id = "marketplace", name = "Marketplace", admin_only = false },
            { id = "weather", name = "Weather Forecast", admin_only = false },
            { id = "profile", name = "Profile Editor", admin_only = false },
            { id = "admin", name = "Admin Console", admin_only = true },
        }
    end

    term.define_form(10)

    local start_col = cfg.start_col or 2
    local start_row = cfg.start_row or 10
    local col_width = cfg.col_width or 16
    local layout = cfg.layout or "grid"

    local current_row = start_row
    local current_col = start_col
    local items_in_col = 0
    local registered_apps = {}

    for _, app_info in ipairs(apps) do
        local can_show = true
        if app_info.admin_only and not is_admin then
            can_show = false
        elseif app_info.required_permission and app_info.required_permission ~= "" then
            if not is_admin and not session.has_permission(app_info.required_permission) then
                can_show = false
            end
        end

        if can_show then
            local button_id = app_info.id
            registered_apps[button_id] = app_info.id

            term.add_submit_button(button_id, current_col, current_row)

            if layout == "grid" then
                items_in_col = items_in_col + 1
                if items_in_col % 3 == 0 then
                    current_col = current_col + col_width
                    current_row = start_row
                else
                    current_row = current_row + 2
                end
            else
                current_row = current_row + 2
            end
        end
    end

    if cfg.show_logout then
        term.add_submit_button("logout", current_col, current_row)
    end

    term.flush_form()

    session.await_input(10, function(submission)
        if type(submission) == "string" then
            menu.on_start(session)
            return
        end

        local action = submission.submit
        log.info("Main menu selected action: " .. tostring(action))

        if action == "logout" then
            log.info("User logged out: " .. user.nickname)
            term.clear()
            term.print("Goodbye, " .. user.nickname .. "!\n")
            term.flush()
            session.close()
        elseif registered_apps[action] then
            session.load_app(registered_apps[action])
        -- Support backward compatibility with legacy button action names
        elseif action == "read_boards" then
            session.load_app("messages")
        elseif action == "door_game" then
            session.load_app("minidungeon")
        else
            -- Direct app name lookup
            local matched = false
            for _, app_info in ipairs(apps) do
                if app_info.id == action then
                    session.load_app(app_info.id)
                    matched = true
                    break
                end
            end
            if not matched then
                menu.on_start(session)
            end
        end
    end)
end

function menu.on_resume(session)
    menu.on_start(session)
end

return menu
