-- plugin-name: Tmux Sessions
-- plugin-description: List and attach to tmux sessions
-- plugin-type: panel
-- plugin-version: 1.1.0
-- plugin-location: right

local sessions = {}
local selected = nil
local last_refresh = 0

function refresh_sessions()
    sessions = {}

    -- Get sessions with window details.
    -- Format: session_name|session_windows|session_attached|session_activity
    local result = session.exec(
        "tmux list-sessions -F '#{session_name}|#{session_windows}|#{session_attached}|#{session_activity}' 2>/dev/null"
    )
    if result.status ~= "ok" or result.stdout == "" then
        return
    end

    local sess_map = {}
    local sess_order = {}

    for line in result.stdout:gmatch("[^\n]+") do
        local name, windows, attached, activity = line:match("^(.+)|(%d+)|(%d+)|(%d+)$")
        if name then
            sess_map[name] = {
                name = name,
                windows = tonumber(windows) or 0,
                attached = tonumber(attached) or 0,
                activity = tonumber(activity) or 0,
                win_list = {},
            }
            table.insert(sess_order, name)
        end
    end

    -- Get windows per session.
    -- Format: session_name|window_index|window_name|window_active|window_panes
    local win_result = session.exec(
        "tmux list-windows -a -F '#{session_name}|#{window_index}|#{window_name}|#{window_active}|#{window_panes}' 2>/dev/null"
    )
    if win_result.status == "ok" and win_result.stdout ~= "" then
        for line in win_result.stdout:gmatch("[^\n]+") do
            local sname, windex, wname, wactive, wpanes =
                line:match("^(.+)|(%d+)|(.+)|(%d+)|(%d+)$")
            if sname and sess_map[sname] then
                table.insert(sess_map[sname].win_list, {
                    index = tonumber(windex) or 0,
                    name = wname or "?",
                    active = (tonumber(wactive) or 0) > 0,
                    panes = tonumber(wpanes) or 1,
                })
            end
        end
    end

    -- Sort sessions: attached first, then by activity (most recent first).
    table.sort(sess_order, function(a, b)
        local sa, sb = sess_map[a], sess_map[b]
        if sa.attached ~= sb.attached then
            return sa.attached > sb.attached
        end
        return sa.activity > sb.activity
    end)

    for _, name in ipairs(sess_order) do
        table.insert(sessions, sess_map[name])
    end
end

function setup()
    app.register_menu_item("Tools", "Tmux Sessions", "show_tmux", "cmd+shift+x")
    refresh_sessions()
end

function on_event(event)
    if type(event) ~= "table" then return end

    if event.action == "show_tmux" then
        refresh_sessions()
    end

    -- Tree node clicked.
    if event.type == "tree_select" and event.id == "tmux_tree" then
        selected = event.node_id
    end

    -- Tree node double-clicked — attach to that session.
    if event.type == "tree_activate" and event.id == "tmux_tree" then
        -- node_id is "sess:<name>" for sessions, "win:<sess>:<idx>" for windows.
        local sess_name = event.node_id:match("^sess:(.+)$")
        local win_sess, win_idx = event.node_id:match("^win:(.+):(%d+)$")

        if sess_name then
            session.new_tab("tmux attach -t " .. sess_name .. "\n", true)
        elseif win_sess and win_idx then
            session.new_tab("tmux attach -t " .. win_sess .. " \\; select-window -t " .. win_idx .. "\n", true)
        end
    end

    -- Context menu actions.
    if event.type == "tree_context_menu" and event.id == "tmux_tree" then
        local sess_name = event.node_id:match("^sess:(.+)$")
        if not sess_name then
            sess_name = event.node_id:match("^win:(.+):%d+$")
        end

        if sess_name then
            if event.action == "rename" then
                local new_name = ui.prompt("Rename session '" .. sess_name .. "' to:")
                if new_name and new_name ~= "" then
                    session.exec("tmux rename-session -t " .. sess_name .. " " .. new_name)
                    refresh_sessions()
                end
            elseif event.action == "kill" then
                if ui.confirm("Kill session '" .. sess_name .. "'?") then
                    session.exec("tmux kill-session -t " .. sess_name)
                    refresh_sessions()
                end
            elseif event.action == "detach" then
                session.exec("tmux detach-client -s " .. sess_name)
                refresh_sessions()
            end
        end
    end

    -- Refresh button.
    if event.type == "button_click" and event.id == "tmux_refresh" then
        refresh_sessions()
    end

    -- New session button.
    if event.type == "button_click" and event.id == "tmux_new" then
        local name = ui.prompt("New tmux session name:")
        if name and name ~= "" then
            session.exec("tmux new-session -d -s " .. name)
            refresh_sessions()
        end
    end

    -- Clean up orphaned (detached) sessions.
    if event.type == "button_click" and event.id == "tmux_cleanup" then
        local count = 0
        for _, s in ipairs(sessions) do
            if s.attached == 0 then
                count = count + 1
            end
        end
        if count == 0 then
            app.notify("Tmux", "No detached sessions to clean up", "info", 3000)
        elseif ui.confirm("Kill " .. count .. " detached session(s)?") then
            for _, s in ipairs(sessions) do
                if s.attached == 0 then
                    session.exec("tmux kill-session -t " .. s.name)
                end
            end
            refresh_sessions()
            app.notify("Tmux", "Killed " .. count .. " detached session(s)", "success", 3000)
        end
    end
end

function render()
    ui.panel_heading("Tmux Sessions")

    ui.panel_toolbar(nil, {
        { type = "button", id = "tmux_refresh", icon = "refresh", tooltip = "Refresh" },
        { type = "button", id = "tmux_new", label = "+", tooltip = "New Session" },
        { type = "button", id = "tmux_cleanup", label = "Clean", tooltip = "Kill all detached sessions" },
    })

    -- Auto-refresh every 10 seconds.
    local now = os.time()
    if now - last_refresh >= 10 then
        refresh_sessions()
        last_refresh = now
    end

    if #sessions == 0 then
        ui.panel_label("No tmux sessions running", "muted")
        ui.panel_spacer(8)
        ui.panel_label("Click + to create one", "muted")
        return
    end

    local attached_count = 0
    local detached_count = 0
    for _, s in ipairs(sessions) do
        if s.attached > 0 then attached_count = attached_count + 1
        else detached_count = detached_count + 1 end
    end

    ui.panel_label(
        attached_count .. " attached, " .. detached_count .. " detached",
        "muted"
    )

    ui.panel_scroll_area(function()
        local nodes = {}
        for _, s in ipairs(sessions) do
            -- Build window children for this session.
            local children = {}
            for _, w in ipairs(s.win_list) do
                local win_label = w.index .. ": " .. w.name
                if w.panes > 1 then
                    win_label = win_label .. " (" .. w.panes .. " panes)"
                end
                table.insert(children, {
                    id = "win:" .. s.name .. ":" .. w.index,
                    label = win_label,
                    bold = w.active,
                    icon = "terminal",
                })
            end

            -- Session node.
            local badge = nil
            local icon_color = "muted"
            if s.attached > 0 then
                badge = "attached"
                icon_color = "blue"
            end

            local context_menu = {
                { id = "rename", label = "Rename Session..." },
                { id = "detach", label = "Detach Clients" },
                { id = "kill", label = "Kill Session" },
            }

            table.insert(nodes, {
                id = "sess:" .. s.name,
                label = s.name,
                badge = badge,
                icon = "terminal",
                icon_color = icon_color,
                bold = s.attached > 0,
                expanded = s.attached > 0,
                children = children,
                context_menu = context_menu,
            })
        end

        ui.panel_tree("tmux_tree", nodes, selected)
    end)

    ui.panel_separator()
    ui.panel_label("Double-click to attach", "muted")
end
