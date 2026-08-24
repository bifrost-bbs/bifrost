local board = {}

local function generate_id()
    local chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    local id = ""
    for i = 1, 8 do
        local r = math.random(1, #chars)
        id = id .. chars:sub(r, r)
    end
    return id
end

function board.show_categories(session)
    term.clear()
    term.set_color(14, 0)
    term.print("=== BIFROST DISCUSSION BOARDS ===\n\n")
    term.set_color(7, 0)
    term.print("Select a category:\n\n")

    local categories = db.get("msg_categories", "list")
    if not categories or #categories == 0 then
        categories = {
            { id = "cat_general", name = "General" },
            { id = "cat_hardware", name = "Hardware" },
            { id = "cat_emergencies", name = "Emergencies" }
        }
        db.set("msg_categories", "list", categories)
    end

    term.define_form(200)

    local y = 5
    for i, cat in ipairs(categories) do
        term.add_submit_button("view_cat_" .. cat.id, 2, y)
        term.print("    [" .. cat.name .. "]\n\n")
        y = y + 2
    end

    if session.has_permission("admin") then
        term.add_submit_button("admin_manage", 2, y)
        term.print("    Manage Categories (Admin)\n\n")
        y = y + 2
    end

    term.add_submit_button("main_menu", 2, y)
    term.print("    Return to Main Menu\n")
    
    term.flush_form()
    
    session.await_input(200, function(submission)
        if type(submission) == "string" then
            local s = submission:lower()
            if s == "m" or s == "q" or s == "b" or s == "x" or s == "exit" or s == "quit" then
                session.load_app("main_menu")
            else
                board.show_categories(session)
            end
            return
        end

        local action = submission.submit
        if action == "main_menu" or action == "back" or action == "exit" or action == "quit" then
            session.load_app("main_menu")
        elseif action == "admin_manage" then
            if session.has_permission("admin") then
                board.manage_categories(session)
            else
                board.show_categories(session)
            end
        elseif action and action:sub(1, 9) == "view_cat_" then
            local cat_id = action:sub(10)
            board.show_category_messages(session, cat_id)
        else
            board.show_categories(session)
        end
    end)
end

function board.manage_categories(session)
    term.clear()
    term.set_color(12, 0)
    term.print("=== MANAGE CATEGORIES ===\n\n")
    term.set_color(7, 0)

    local categories = db.get("msg_categories", "list") or {}

    term.print("Current Categories:\n")
    local y = 5
    for i, cat in ipairs(categories) do
        term.print(string.format("  %d. %s (ID: %s)\n", i, cat.name, cat.id))
        y = y + 1
    end

    term.print("\nAdd New Category:\n")
    y = y + 2

    term.define_form(201)
    term.add_input_field("new_cat_name", 2, y, 20, "")
    term.add_submit_button("add_cat", 24, y)

    y = y + 2
    term.print("\nDelete Category (by ID):\n")
    y = y + 2
    term.add_input_field("del_cat_id", 2, y, 15, "")
    term.add_submit_button("del_cat", 19, y)

    y = y + 2
    term.add_submit_button("back", 2, y)
    term.print("    Back to Categories\n")

    term.flush_form()

    session.await_input(201, function(submission)
        if type(submission) == "string" then
            board.manage_categories(session)
            return
        end

        local action = submission.submit
        if action == "back" then
            board.show_categories(session)
        elseif action == "add_cat" then
            local new_name = submission.new_cat_name or ""
            if new_name ~= "" then
                local new_id = "cat_" .. generate_id()
                table.insert(categories, { id = new_id, name = new_name })
                db.set("msg_categories", "list", categories)
            end
            board.manage_categories(session)
        elseif action == "del_cat" then
            local del_id = submission.del_cat_id or ""
            if del_id ~= "" then
                local idx = nil
                for i, cat in ipairs(categories) do
                    if cat.id == del_id then
                        idx = i
                        break
                    end
                end
                if idx then
                    table.remove(categories, idx)
                    db.set("msg_categories", "list", categories)
                end
            end
            board.manage_categories(session)
        else
            board.manage_categories(session)
        end
    end)
end

function board.show_category_messages(session, cat_id)
    local categories = db.get("msg_categories", "list") or {}
    local cat_name = "Unknown"
    for _, cat in ipairs(categories) do
        if cat.id == cat_id then
            cat_name = cat.name
            break
        end
    end

    term.clear()
    term.set_color(14, 0)
    term.print("=== " .. string.upper(cat_name) .. " MESSAGES ===\n\n")
    term.set_color(7, 0)

    local messages = db.get("messages", cat_id) or {}
    -- Seed some initial messages if empty just for flavor, similar to old hardcoded version
    if #messages == 0 then
        if cat_id == "cat_general" then
            table.insert(messages, { id = "msg_1", subject = "Welcome to the Mesh!", author = "g8way", body = "This is a decentralized mesh BBS running on Bifrost!" })
        elseif cat_id == "cat_hardware" then
            table.insert(messages, { id = "msg_2", subject = "LilyGO T-Deck tips", author = "radio_fan", body = "Keep your Spreading Factor low to minimize airtime." })
        elseif cat_id == "cat_emergencies" then
             table.insert(messages, { id = "msg_3", subject = "Water station locations", author = "coordinator", body = "Water stations are active at Sector 4 and 7." })
        end
        db.set("messages", cat_id, messages)
    end

    if #messages == 0 then
        term.print("No messages in this category.\n\n")
    else
        term.print("Select a message:\n\n")
    end

    term.define_form(202)
    local y = 5
    for i, msg in ipairs(messages) do
        term.add_submit_button("read_" .. msg.id, 2, y)
        term.print("    " .. msg.subject .. " (by " .. msg.author .. ")\n")
        y = y + 1
    end

    y = y + 2
    term.add_submit_button("post_msg", 2, y)
    term.print("    Post a new message\n")
    y = y + 2

    term.add_submit_button("back", 2, y)
    term.print("    Back to Categories\n")

    term.flush_form()

    session.await_input(202, function(submission)
        if type(submission) == "string" then
            board.show_category_messages(session, cat_id)
            return
        end

        local action = submission.submit
        if action == "back" then
            board.show_categories(session)
        elseif action == "post_msg" then
            board.post_message(session, cat_id)
        elseif action and action:sub(1, 5) == "read_" then
            local msg_id = action:sub(6)
            board.view_message(session, cat_id, msg_id)
        else
            board.show_category_messages(session, cat_id)
        end
    end)
end

function board.post_message(session, cat_id)
    term.clear()
    term.set_color(14, 0)
    term.print("=== POST NEW MESSAGE ===\n\n")
    term.set_color(7, 0)

    term.print("Subject:\n")
    term.print("\nBody:\n")

    term.define_form(203)
    term.add_input_field("subject", 2, 4, 30, "")
    -- Since we only have single line inputs easily mapped to simple terminal macros,
    -- we'll use a few lines to form a small body or just one long line. Let's use 2 lines.
    term.add_input_field("body_l1", 2, 7, 35, "")
    term.add_input_field("body_l2", 2, 8, 35, "")

    term.add_submit_button("submit_post", 2, 11)
    term.print("    Submit Post\n")

    term.add_submit_button("cancel", 20, 11)
    term.print("    Cancel\n")

    term.flush_form()

    session.await_input(203, function(submission)
        if type(submission) == "string" then
            board.post_message(session, cat_id)
            return
        end

        local action = submission.submit
        if action == "cancel" then
            board.show_category_messages(session, cat_id)
        elseif action == "submit_post" then
            local user = db.get("users", session.node_id())
            local author = user and user.nickname or "Operator"

            local subject = submission.subject or "No Subject"
            local body1 = submission.body_l1 or ""
            local body2 = submission.body_l2 or ""
            local body = body1
            if body2 ~= "" then
                body = body .. "\n" .. body2
            end
            
            local messages = db.get("messages", cat_id) or {}
            table.insert(messages, {
                id = "msg_" .. generate_id(),
                subject = subject,
                author = author,
                body = body
            })
            db.set("messages", cat_id, messages)
            
            board.show_category_messages(session, cat_id)
        else
            board.post_message(session, cat_id)
        end
    end)
end

function board.view_message(session, cat_id, msg_id)
    local messages = db.get("messages", cat_id) or {}
    local msg = nil
    for _, m in ipairs(messages) do
        if m.id == msg_id then
            msg = m
            break
        end
    end

    term.clear()
    term.set_color(14, 0)
    term.print("=== MESSAGE VIEWER ===\n\n")
    term.set_color(7, 0)

    if msg then
        term.print("Subject: " .. msg.subject .. "\n")
        term.print("From: " .. msg.author .. "\n\n")
        term.print(msg.body .. "\n")
    else
        term.print("Message not found.\n")
    end

    term.print("\n\n")

    term.define_form(204)
    term.add_submit_button("back", 2, 10)
    term.print("    Back to message list\n")
    term.flush_form()

    session.await_input(204, function() board.show_category_messages(session, cat_id) end)
end

function board.on_start(session)
    log.info("Discussion Boards application loaded.")
    board.show_categories(session)
end

function board.on_resume(session)
    board.show_categories(session)
end

return board
