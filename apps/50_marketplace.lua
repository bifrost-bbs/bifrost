local market = {}

-- Schema:
-- categories: array of category names
-- category_<category_name>_items: array of items
-- item: { id, title, desc, price, seller_id, offers: array of {buyer_id, amount} }

function market.on_start(session)
    local is_admin = session.has_permission("admin")

    term.clear()
    term.set_color(10, 0) -- Light Green on Black
    term.print("=== BIFROST MARKETPLACE ===\n\n")
    term.set_color(7, 0)

    local categories = db.get("marketplace", "categories") or { "General" }

    term.print("Select a category:\n\n")
    term.define_form(50)

    local y = 5
    for i, cat in ipairs(categories) do
        term.add_submit_button("cat_" .. tostring(i), 2, y)
        term.print("    " .. cat .. "\n\n")
        y = y + 2
    end

    if is_admin then
        term.add_submit_button("admin_new_cat", 2, y)
        term.print("    [Admin] Add Category\n\n")
        y = y + 2
    end

    term.add_submit_button("main_menu", 2, y)
    term.print("    Return to Main Menu\n")
    term.flush_form()

    session.await_input(50, function(submission)
        if type(submission) == "string" then
            market.on_start(session)
            return
        end

        local action = submission.submit
        if action == "main_menu" then
            session.load_app("00_main_menu")
        elseif action == "admin_new_cat" and is_admin then
            market.new_category(session)
        elseif string.sub(action, 1, 4) == "cat_" then
            local idx = tonumber(string.sub(action, 5))
            if idx and categories[idx] then
                market.view_category(session, categories[idx])
            else
                market.on_start(session)
            end
        else
            market.on_start(session)
        end
    end)
end

function market.new_category(session)
    term.clear()
    term.set_color(10, 0)
    term.print("=== ADD NEW CATEGORY ===\n\n")
    term.set_color(7, 0)

    term.define_form(51)
    term.print("  Category Name: ")
    term.add_input_field("cat_name", 18, 4, 20, "")
    term.print("\n\n")

    term.add_submit_button("save", 2, 7)
    term.add_submit_button("cancel", 12, 7)
    term.flush_form()

    session.await_input(51, function(submission)
        if type(submission) == "string" then
            market.new_category(session)
            return
        end
        if submission.submit == "save" then
            local new_cat = submission.cat_name or ""
            if new_cat ~= "" then
                local categories = db.get("marketplace", "categories") or { "General" }
                table.insert(categories, new_cat)
                db.set("marketplace", "categories", categories)
            end
        end
        market.on_start(session)
    end)
end

function market.view_category(session, cat_name)
    term.clear()
    term.set_color(10, 0)
    term.print("=== CATEGORY: " .. cat_name .. " ===\n\n")
    term.set_color(7, 0)

    local items = db.get("market_items", cat_name) or {}

    term.define_form(52)
    local y = 4

    if #items == 0 then
        term.print("  No items in this category yet.\n\n")
        y = y + 2
    else
        for i, item in ipairs(items) do
            term.add_submit_button("item_" .. tostring(i), 2, y)
            term.print("    " .. item.title .. " (" .. tostring(item.price) .. " credits)\n\n")
            y = y + 2
        end
    end

    term.add_submit_button("new_item", 2, y)
    term.print("    List New Item\n\n")
    y = y + 2

    term.add_submit_button("back", 2, y)
    term.print("    Back to Categories\n")
    term.flush_form()

    session.await_input(52, function(submission)
        if type(submission) == "string" then
            market.view_category(session, cat_name)
            return
        end
        local action = submission.submit
        if action == "back" then
            market.on_start(session)
        elseif action == "new_item" then
            market.new_item(session, cat_name)
        elseif string.sub(action, 1, 5) == "item_" then
            local idx = tonumber(string.sub(action, 6))
            if idx and items[idx] then
                market.view_item(session, cat_name, idx)
            else
                market.view_category(session, cat_name)
            end
        else
            market.view_category(session, cat_name)
        end
    end)
end

function market.new_item(session, cat_name)
    term.clear()
    term.set_color(10, 0)
    term.print("=== LIST NEW ITEM IN " .. cat_name .. " ===\n\n")
    term.set_color(7, 0)

    term.define_form(53)
    term.print("  Title: ")
    term.add_input_field("title", 10, 4, 20, "")
    term.print("\n\n")
    term.print("  Price: ")
    term.add_input_field("price", 10, 6, 10, "")
    term.print("\n\n")
    term.print("  Description:\n")
    term.add_multiline_field("desc", 2, 9, 36, 4, "")
    term.print("\n\n\n\n\n")

    term.add_submit_button("save", 2, 16)
    term.add_submit_button("cancel", 12, 16)
    term.flush_form()

    session.await_input(53, function(submission)
        if type(submission) == "string" then
            market.new_item(session, cat_name)
            return
        end
        if submission.submit == "save" then
            local title = submission.title or ""
            local price = tonumber(submission.price) or 0
            local desc = submission.desc or ""
            if title ~= "" then
                local items = db.get("market_items", cat_name) or {}
                table.insert(items, {
                    title = title,
                    price = price,
                    desc = desc,
                    seller_id = session.node_id(),
                    offers = {}
                })
                db.set("market_items", cat_name, items)
            end
        end
        market.view_category(session, cat_name)
    end)
end

function market.view_item(session, cat_name, item_idx)
    local items = db.get("market_items", cat_name) or {}
    local item = items[item_idx]
    if not item then
        market.view_category(session, cat_name)
        return
    end

    term.clear()
    term.set_color(10, 0)
    term.print("=== " .. item.title .. " ===\n\n")
    term.set_color(7, 0)

    local seller = db.get("users", item.seller_id)
    local seller_nick = seller and seller.nickname or "Unknown"

    term.print("Seller: " .. seller_nick .. "\n")
    term.print("Asking Price: " .. tostring(item.price) .. " credits\n\n")
    term.print("Description:\n")
    term.print(item.desc .. "\n\n")

    term.print("Offers:\n")
    if #item.offers == 0 then
        term.print("  None yet.\n")
    else
        for i, offer in ipairs(item.offers) do
            local buyer = db.get("users", offer.buyer_id)
            local buyer_nick = buyer and buyer.nickname or "Unknown"
            term.print("  " .. buyer_nick .. ": " .. tostring(offer.amount) .. " credits\n")
        end
    end
    term.print("\n")

    term.define_form(54)
    term.add_submit_button("make_offer", 2, 18)
    term.print("    Make an Offer\n")

    term.add_submit_button("back", 22, 18)
    term.print("    Back\n")
    term.flush_form()

    session.await_input(54, function(submission)
        if type(submission) == "string" then
            market.view_item(session, cat_name, item_idx)
            return
        end
        local action = submission.submit
        if action == "make_offer" then
            market.make_offer(session, cat_name, item_idx)
        else
            market.view_category(session, cat_name)
        end
    end)
end

function market.make_offer(session, cat_name, item_idx)
    local items = db.get("market_items", cat_name) or {}
    local item = items[item_idx]
    if not item then
        market.view_category(session, cat_name)
        return
    end

    term.clear()
    term.set_color(10, 0)
    term.print("=== MAKE OFFER ON: " .. item.title .. " ===\n\n")
    term.set_color(7, 0)

    term.define_form(55)
    term.print("  Offer Amount: ")
    term.add_input_field("amount", 16, 4, 10, "")
    term.print("\n\n")

    term.add_submit_button("submit", 2, 7)
    term.add_submit_button("cancel", 12, 7)
    term.flush_form()

    session.await_input(55, function(submission)
        if type(submission) == "string" then
            market.make_offer(session, cat_name, item_idx)
            return
        end
        if submission.submit == "submit" then
            local amount = tonumber(submission.amount)
            if amount then
                -- Must re-fetch in case it changed
                local current_items = db.get("market_items", cat_name) or {}
                if current_items[item_idx] then
                    table.insert(current_items[item_idx].offers, {
                        buyer_id = session.node_id(),
                        amount = amount
                    })
                    db.set("market_items", cat_name, current_items)
                end
            end
        end
        market.view_item(session, cat_name, item_idx)
    end)
end

return market
