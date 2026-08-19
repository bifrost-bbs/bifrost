local weather = {}

function weather.on_start(session)
    local user_id = session.node_id()
    local user = db.get("users", user_id)

    term.clear()
    term.move_to(2, 2)
    term.set_color(11, 0) -- Yellow on Black
    term.print("WEATHER FORECAST\n\n")
    term.set_color(7, 0)

    if not user or not user.latitude or not user.longitude then
        term.print("No location data found for your node.\n")
        term.print("Please ensure your radio advertises GPS coordinates.\n\n")

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

    term.print(string.format("Fetching forecast for Location: %.4f, %.4f...\n\n", lat, lon))
    term.flush()

    local url = string.format("https://api.open-meteo.com/v1/forecast?latitude=%f&longitude=%f&current_weather=true", lat, lon)
    local data = http.get_json(url)

    if not data or not data.current_weather then
        term.print("Failed to fetch weather data from API.\n\n")
    else
        local cw = data.current_weather
        term.print(string.format("Temperature: %.1f C\n", cw.temperature))
        term.print(string.format("Wind Speed: %.1f km/h\n", cw.windspeed))
        term.print(string.format("Wind Direction: %d degrees\n", cw.winddirection))
        -- Add mapping for WMO weather code if possible, or just print code
        term.print(string.format("Weather Code (WMO): %d\n", cw.weathercode))
    end

    term.print("\n")
    term.define_form(2)
    term.add_submit_button("back", 2, 14)
    term.flush_form()

    session.await_input(2, function(submission)
        session.load_app("main_menu")
    end)
end

return weather
