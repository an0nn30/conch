-- plugin-name: Docked View Example
-- plugin-description: Example Lua plugin demonstrating docked views and view-scoped widget events
-- plugin-version: 0.1.0
-- plugin-type: action
-- plugin-api: ^1.0
-- plugin-permissions: ui.menu, ui.panel, ui.dock, ui.notify

local ACTION_OPEN = "docked_view_example_open"
local ACTION_FOCUS = "docked_view_example_focus"
local ACTION_CLOSE = "docked_view_example_close"
local STABLE_VIEW_ID = "example"

local active_view_id = nil
local counts_by_view = {}

local function ensure_count(view_id)
    if view_id == nil or view_id == "" then
        return 0
    end
    if counts_by_view[view_id] == nil then
        counts_by_view[view_id] = 0
    end
    return counts_by_view[view_id]
end

local function open_or_focus()
    if active_view_id ~= nil and ui.focus_docked_view(active_view_id) then
        return active_view_id
    end

    local result = ui.open_docked_view({
        id = STABLE_VIEW_ID,
        title = "Docked View Example",
        icon = "activity",
        dock = {
            direction = "horizontal",
            ratio = 0.35,
        },
    })

    if type(result) ~= "table" or type(result.view_id) ~= "string" then
        app.notify("Docked View", "Failed to open docked view", "error", 3000)
        return nil
    end

    active_view_id = result.view_id
    ensure_count(active_view_id)
    app.notify("Docked View", "Opened " .. active_view_id, "success", 1800)
    return active_view_id
end

function setup()
    app.register_command("Plugins", "Docked View Example: Open", ACTION_OPEN, nil)
    app.register_command("Plugins", "Docked View Example: Focus", ACTION_FOCUS, nil)
    app.register_command("Plugins", "Docked View Example: Close", ACTION_CLOSE, nil)
end

function on_event(event)
    if type(event) ~= "table" then
        return
    end

    if event.action == ACTION_OPEN then
        open_or_focus()
        return
    end

    if event.action == ACTION_FOCUS then
        if active_view_id ~= nil and ui.focus_docked_view(active_view_id) then
            app.notify("Docked View", "Focused " .. active_view_id, "info", 1400)
        else
            open_or_focus()
        end
        return
    end

    if event.action == ACTION_CLOSE then
        if active_view_id ~= nil and ui.close_docked_view(active_view_id) then
            app.notify("Docked View", "Closed " .. active_view_id, "info", 1400)
            active_view_id = nil
        else
            app.notify("Docked View", "No open docked view to close", "warn", 1800)
        end
        return
    end

    if event.kind ~= "widget" then
        return
    end

    local view_id = event.view_id
    if type(view_id) ~= "string" or view_id == "" then
        view_id = active_view_id
    end
    if type(view_id) ~= "string" or view_id == "" then
        return
    end

    if event.id == "inc" then
        local next_count = ensure_count(view_id) + 1
        counts_by_view[view_id] = next_count
        app.notify("Docked View", "Count(" .. view_id .. ") = " .. tostring(next_count), "info", 1200)
    elseif event.id == "reset" then
        counts_by_view[view_id] = 0
        app.notify("Docked View", "Count reset for " .. view_id, "warn", 1200)
    elseif event.id == "close" then
        ui.close_docked_view(view_id)
        if view_id == active_view_id then
            active_view_id = nil
        end
    end
end

function render_view(view_id)
    local count = ensure_count(view_id)
    ui.panel_heading("Docked View Example")
    ui.panel_label("view_id: " .. tostring(view_id), "muted")
    ui.panel_label("Use buttons to verify view-scoped widget event routing.", "secondary")
    ui.panel_separator()
    ui.panel_kv("Counter", tostring(count))
    ui.panel_horizontal(function()
        ui.panel_button("inc", "Increment")
        ui.panel_button("reset", "Reset")
        ui.panel_button("close", "Close")
    end, 8)
end
