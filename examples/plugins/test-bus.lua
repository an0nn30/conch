-- plugin-name: Bus Test
-- plugin-description: Tests inter-plugin bus communication
-- plugin-type: panel
-- plugin-version: 0.1.0
-- plugin-location: bottom

local received_events = {}
local publish_count = 0

function setup()
    app.log("info", "Bus test plugin loaded")

    -- Subscribe to test events and SSH events.
    app.subscribe("test.ping")
    app.subscribe("test.echo")
    app.subscribe("ssh.session_ready")
    app.subscribe("ssh.session_closed")
end

function render()
    ui.panel_clear()

    ui.panel_heading("Bus Communication Test")

    -- Publishing controls
    ui.panel_kv("Published:", tostring(publish_count))
    ui.panel_button("ping", "Publish test.ping")
    ui.panel_button("echo", "Publish test.echo")

    ui.panel_separator()

    -- Received events log
    ui.panel_heading("Received Events")
    if #received_events == 0 then
        ui.panel_label("No events received yet.", "muted")
    else
        ui.panel_table({"#", "Type", "Data"}, received_events)
    end

    ui.panel_separator()
    ui.panel_button("clear_log", "Clear Event Log")
end

function on_event(event)
    if event.type == "button_click" then
        if event.id == "ping" then
            app.publish("test.ping", { sender = "bus-test", seq = publish_count })
            publish_count = publish_count + 1
        elseif event.id == "echo" then
            app.publish("test.echo", { message = "hello from bus test", seq = publish_count })
            publish_count = publish_count + 1
        elseif event.id == "clear_log" then
            received_events = {}
        end
    elseif event.type == "bus_event" then
        local idx = #received_events + 1
        local data_str = tostring(event.data or "{}")
        table.insert(received_events, {tostring(idx), event.event_type or "?", data_str})
        app.log("info", "Received bus event: " .. (event.event_type or "unknown"))
    end
end
