local weather = {}

function weather.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0) -- Cyan
    term.print("=== WEATHER FORECAST ===\n\n")
    term.set_color(7, 0)

    if not user or not user.latitude or not user.longitude then
        term.print("No location coordinates found for your radio node.\n")
        term.print("Please ensure your radio advertises GPS coordinates in MeshCore adverts.\n\n")

        term.define_form(1)
        term.add_submit_button("back", 2, 8)
        term.flush_form()

        session.await_input(1, function(submission)
            session.load_app("main_menu")
        end)
        return
    end

    local lat = user.latitude
    local lon = user.longitude

    term.print(string.format("Location: %.4f, %.4f\n", lat, lon))

    local url = string.format("https://api.open-meteo.com/v1/forecast?latitude=%f&longitude=%f&current_weather=true", lat, lon)
    local data = http.get_json(url)

    if not data or not data.current_weather then
        term.print("Failed to fetch weather data from API.\n\n")
    else
        local cw = data.current_weather
        term.render_table(2, 6, {
            headers = { "Metric", "Reading" },
            widths = { 20, 20 },
            rows = {
                { "Temperature", string.format("%.1f C", cw.temperature) },
                { "Wind Speed", string.format("%.1f km/h", cw.windspeed) },
                { "Wind Direction", string.format("%d deg", cw.winddirection) },
                { "Condition (WMO)", tostring(cw.weathercode) }
            },
            header_fg = 14,
            row_fg = 15,
            divider = true
        })
    end

    term.define_form(2)
    term.add_submit_button("back", 2, 14)
    term.flush_form()

    session.await_input(2, function(submission)
        session.load_app("main_menu")
    end)
end

function weather.on_resume(session)
    weather.on_start(session)
end

return weather
