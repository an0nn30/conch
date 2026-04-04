-- plugin-name: Tmux Manager
-- plugin-description: Manage local tmux sessions from a docked tool window and command palette actions.
-- plugin-version: 1.5.0
-- plugin-api: ^1.0
-- plugin-permissions: ui.panel, ui.menu, ui.notify, ui.dialog, session.exec, session.new_tab, session.rename_tab, bus.subscribe
-- plugin-type: tool_window
-- plugin-location: right
-- plugin-keybind: tmux_refresh = cmd+shift+alt+t | Refresh Tmux sessions

local ACTION_REFRESH = "tmux_refresh"
local ACTION_ATTACH_EXISTING = "tmux_attach_existing"
local ACTION_CREATE_NEW = "tmux_create_new"
local ACTION_RENAME_EXISTING = "tmux_rename_existing"
local ACTION_DELETE_EXISTING = "tmux_delete_existing"

local state = {
    sessions = {},
    status = "Loading tmux sessions...",
    last_error = nil,
    current_session = nil,
    attached_anywhere = 0,
    attached_total_clients = 0,
    last_refresh_unix = 0,
    next_poll_unix = 0,
    action_targets = {},
    tracked_tabs = {},
    pending_tabs_by_name = {},
    registered_attach_actions = {},
    attach_action_targets = {},
}

local function trim(s)
    return (s or ""):match("^%s*(.-)%s*$")
end

local function sh_quote(s)
    return "'" .. tostring(s or ""):gsub("'", "'\"'\"'") .. "'"
end

local function split_lines(text)
    local out = {}
    if text == nil or text == "" then
        return out
    end
    for line in (text .. "\n"):gmatch("(.-)\n") do
        out[#out + 1] = line
    end
    return out
end

local function split_tab(line)
    local out = {}
    local rest = tostring(line or "")
    while true do
        local idx = rest:find("\t", 1, true)
        if idx == nil then
            out[#out + 1] = rest
            break
        end
        out[#out + 1] = rest:sub(1, idx - 1)
        rest = rest:sub(idx + 1)
    end
    return out
end

local function html_escape(s)
    local v = tostring(s or "")
    v = v:gsub("&", "&amp;")
    v = v:gsub("<", "&lt;")
    v = v:gsub(">", "&gt;")
    v = v:gsub('"', "&quot;")
    return v
end

local function now_unix()
    return math.floor(tonumber(net.time() or 0) or 0)
end

local function run_shell(command)
    return session.exec_local(command)
end

local function is_command_missing(result)
    local stderr = tostring((result and result.stderr) or "")
    return stderr:find("command not found", 1, true) ~= nil
        or stderr:find("No such file or directory", 1, true) ~= nil
end

local function tmux_available()
    local result = run_shell("tmux -V")
    if tonumber(result.exit_code or -1) == 0 then
        return true
    end
    if is_command_missing(result) then
        return false
    end
    return false
end

local function run_tmux(args)
    return run_shell("tmux " .. args)
end

local function session_by_name(name)
    for _, s in ipairs(state.sessions or {}) do
        if s.name == name then
            return s
        end
    end
    return nil
end

local function attach_action_for_name(name)
    local raw = tostring(name or "")
    local encoded = raw:gsub("[^%w_%-]", function(ch)
        return string.format("_%02x", string.byte(ch))
    end)
    return "tmux_attach_session__" .. encoded
end

local function sync_session_attach_commands()
    for _, s in ipairs(state.sessions or {}) do
        local action = attach_action_for_name(s.name)
        state.attach_action_targets[action] = s.name
        if not state.registered_attach_actions[action] then
            app.register_command("Tmux: Attach " .. s.name, action)
            state.registered_attach_actions[action] = true
        end
    end
end

local function launch_tmux_in_plain_tab(args, tab_title)
    -- Prevent "sessions should be nested with care" when Conch inherits TMUX.
    local cmd = "env -u TMUX tmux " .. args .. "\n"
    state.status = "Launching: " .. cmd:gsub("\n$", "")
    return session.new_tab_with_title(cmd, true, tab_title)
end

local function tracked_tab_id_for_session(name)
    local known = session_by_name(name)
    if known ~= nil and tonumber(known.created_unix or 0) > 0 then
        local tracked = state.tracked_tabs[tostring(known.created_unix)]
        if tracked ~= nil and tracked.tab_id ~= nil and tracked.tab_id ~= "" then
            return tostring(tracked.tab_id)
        end
    end

    local pending = state.pending_tabs_by_name[name]
    if pending ~= nil and pending ~= "" then
        return tostring(pending)
    end
    return nil
end

local function is_no_server(result)
    if result == nil then
        return false
    end
    local stderr = tostring(result.stderr or "")
    return stderr:find("no server running", 1, true) ~= nil
        or stderr:find("failed to connect to server", 1, true) ~= nil
        or stderr:find("can't find session", 1, true) ~= nil
end

local function detect_current_session_best_effort(sessions)
    local current_name = nil

    -- If Conch itself is launched inside a tmux client, tmux can resolve #S.
    local probe = run_tmux("display-message -p '#S'")
    if tonumber(probe.exit_code or -1) == 0 then
        local value = trim(probe.stdout or "")
        if value ~= "" then
            current_name = value
        end
    end

    if current_name ~= nil then
        return current_name
    end

    -- Fallback heuristic: if exactly one session has attached clients, use it.
    local single = nil
    for _, s in ipairs(sessions or {}) do
        if (tonumber(s.attached_clients or 0) or 0) > 0 then
            if single ~= nil then
                return nil
            end
            single = s.name
        end
    end
    return single
end

local function reconcile_tracked_tabs(sessions)
    local by_created = {}
    for _, s in ipairs(sessions or {}) do
        local key = tostring(s.created_unix or 0)
        if key ~= "0" then
            by_created[key] = s
        end
    end

    -- Bind tabs opened before the created_unix was known.
    for pending_name, tab_id in pairs(state.pending_tabs_by_name or {}) do
        local found = nil
        for _, s in ipairs(sessions or {}) do
            if s.name == pending_name and tonumber(s.created_unix or 0) > 0 then
                found = s
                break
            end
        end
        if found ~= nil then
            local key = tostring(found.created_unix)
            state.tracked_tabs[key] = { tab_id = tab_id, last_name = found.name }
            state.pending_tabs_by_name[pending_name] = nil
        end
    end

    -- Keep tab titles in sync with external tmux renames.
    for created_key, tracked in pairs(state.tracked_tabs or {}) do
        local live = by_created[created_key]
        if live == nil then
            state.tracked_tabs[created_key] = nil
        elseif tracked ~= nil and tracked.tab_id ~= nil and live.name ~= tracked.last_name then
            session.rename_tab_by_id(tracked.tab_id, live.name)
            tracked.last_name = live.name
        end
    end
end

local function refresh_sessions(quiet, update_status)
    quiet = quiet == true
    update_status = update_status ~= false
    state.last_error = nil
    state.current_session = nil
    state.action_targets = {}
    state.attached_anywhere = 0
    state.attached_total_clients = 0
    state.last_refresh_unix = now_unix()

    if not tmux_available() then
        state.sessions = {}
        local message = "tmux is not installed or not available on PATH."
        if update_status then
            state.status = message
        end
        state.last_error = message
        if not quiet then
            app.notify("Tmux Manager", message, "error", 3200)
        end
        return false
    end

    local list = run_tmux("list-sessions -F '#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}\t#{session_activity}'")
    if tonumber(list.exit_code or -1) ~= 0 then
        if is_no_server(list) then
            state.sessions = {}
            if update_status then
                state.status = "No tmux sessions yet."
            end
            return true
        end
        state.sessions = {}
        state.last_error = trim(list.stderr or "Unknown tmux error")
        if update_status then
            state.status = "Failed to list tmux sessions."
        end
        if not quiet then
            app.notify("Tmux Manager", state.last_error, "error", 3800)
        end
        return false
    end

    local parsed = {}
    for _, line in ipairs(split_lines(list.stdout or "")) do
        if trim(line) ~= "" then
            local fields = split_tab(line)
            local attached_clients = tonumber(fields[3] or "0") or 0
            parsed[#parsed + 1] = {
                name = fields[1] or "",
                windows = tonumber(fields[2] or "0") or 0,
                attached_clients = attached_clients,
                created_unix = tonumber(fields[4] or "0") or 0,
                activity_unix = tonumber(fields[5] or "0") or 0,
            }
            if attached_clients > 0 then
                state.attached_anywhere = state.attached_anywhere + 1
                state.attached_total_clients = state.attached_total_clients + attached_clients
            end
        end
    end

    table.sort(parsed, function(a, b)
        return tostring(a.name):lower() < tostring(b.name):lower()
    end)

    state.sessions = parsed
    state.current_session = detect_current_session_best_effort(parsed)
    reconcile_tracked_tabs(parsed)
    sync_session_attach_commands()

    if update_status then
        if #parsed == 0 then
            state.status = "No tmux sessions yet."
        else
            state.status = "Loaded " .. tostring(#parsed) .. " tmux session(s)."
        end
    end

    return true
end

local function sessions_fingerprint()
    local parts = {
        state.current_session or "",
        state.last_error or "",
        tostring(#(state.sessions or {})),
    }
    for _, s in ipairs(state.sessions or {}) do
        parts[#parts + 1] = table.concat({
            s.name or "",
            tostring(s.windows or 0),
            tostring(s.attached_clients or 0),
            tostring(s.created_unix or 0),
            tostring(s.activity_unix or 0),
        }, "|")
    end
    return table.concat(parts, "||")
end

local function poll_tmux_updates(now_unix_value)
    local now_value = tonumber(now_unix_value or 0) or 0
    if now_value <= 0 then
        now_value = now_unix()
    end
    if now_value < (tonumber(state.next_poll_unix or 0) or 0) then
        return
    end
    state.next_poll_unix = now_value + 2

    local before = sessions_fingerprint()
    refresh_sessions(true, false)
    local after = sessions_fingerprint()
    if before ~= after then
        state.status = "Detected external tmux changes and refreshed."
        render()
        ui.request_render()
    end
end

local function attach_session(name)
    name = trim(name)
    if name == "" then
        app.notify("Tmux Manager", "Session name is required.", "warn", 2400)
        return
    end

    local existing_tab_id = tracked_tab_id_for_session(name)
    if existing_tab_id ~= nil then
        session.focus_tab_by_id(existing_tab_id)
        state.status = "Switched to existing tab for tmux session '" .. name .. "'."
        return
    end

    local tab_id = launch_tmux_in_plain_tab("attach-session -t " .. sh_quote(name), name)
    if tab_id ~= nil and tab_id ~= "" then
        local known = session_by_name(name)
        if known ~= nil and tonumber(known.created_unix or 0) > 0 then
            state.tracked_tabs[tostring(known.created_unix)] = { tab_id = tab_id, last_name = name }
        else
            state.pending_tabs_by_name[name] = tab_id
        end
    end
    state.status = "Opening tmux session '" .. name .. "' in a new tab..."
end

local function create_session(name)
    name = trim(name)
    if name == "" then
        app.notify("Tmux Manager", "Session name cannot be empty.", "warn", 2600)
        return
    end

    local tab_id = launch_tmux_in_plain_tab("new-session -s " .. sh_quote(name), name)
    if tab_id ~= nil and tab_id ~= "" then
        state.pending_tabs_by_name[name] = tab_id
    end
    state.status = "Creating session '" .. name .. "' in a new tab..."
    app.notify("Tmux Manager", "Creating session '" .. name .. "' in a new tab.", "success", 2400)
end

local function rename_session(old_name, new_name)
    old_name = trim(old_name)
    new_name = trim(new_name)

    if old_name == "" or new_name == "" then
        app.notify("Tmux Manager", "Both old and new names are required.", "warn", 2600)
        return false
    end

    local result = run_tmux(
        "rename-session -t " .. sh_quote(old_name) .. " " .. sh_quote(new_name)
    )
    if tonumber(result.exit_code or -1) ~= 0 then
        local message = trim(result.stderr or "Unable to rename session.")
        state.status = "Rename failed."
        state.last_error = message
        app.notify("Tmux Manager", message, "error", 3800)
        return false
    end

    state.status = "Renamed '" .. old_name .. "' to '" .. new_name .. "'."
    for _, tracked in pairs(state.tracked_tabs or {}) do
        if tracked.last_name == old_name then
            tracked.last_name = new_name
            if tracked.tab_id ~= nil then
                session.rename_tab_by_id(tracked.tab_id, new_name)
            end
        end
    end
    app.notify("Tmux Manager", state.status, "success", 2200)
    return true
end

local function delete_session(name)
    name = trim(name)
    if name == "" then
        app.notify("Tmux Manager", "Session name is required.", "warn", 2600)
        return false
    end

    local result = run_tmux("kill-session -t " .. sh_quote(name))
    if tonumber(result.exit_code or -1) ~= 0 then
        local message = trim(result.stderr or "Unable to delete session.")
        state.status = "Delete failed."
        state.last_error = message
        app.notify("Tmux Manager", message, "error", 3800)
        return false
    end

    state.status = "Deleted session '" .. name .. "'."
    app.notify("Tmux Manager", state.status, "success", 2200)
    return true
end

local function session_names()
    local names = {}
    for _, s in ipairs(state.sessions or {}) do
        names[#names + 1] = s.name
    end
    return names
end

local function pick_session(title)
    local names = session_names()
    if #names == 0 then
        app.notify("Tmux Manager", "No tmux sessions available.", "warn", 2600)
        return nil
    end

    local response = ui.form(title, {
        { type = "combo", id = "session", label = "Session", value = names[1], options = names },
    })

    if response == nil or response.session == nil then
        return nil
    end
    return trim(response.session)
end

local function prompt_create_session()
    local name = ui.prompt("New tmux session name", "work")
    if name == nil then
        return
    end
    create_session(name)
end

local function prompt_attach_session()
    refresh_sessions(true)
    local name = pick_session("Attach To Tmux Session")
    if name ~= nil and name ~= "" then
        attach_session(name)
    end
end

local function prompt_rename_session(default_name)
    refresh_sessions(true)
    local names = session_names()
    if #names == 0 then
        app.notify("Tmux Manager", "No tmux sessions available.", "warn", 2600)
        return
    end

    local selected = default_name
    if selected == nil or selected == "" then
        selected = names[1]
    end

    local response = ui.form("Rename Tmux Session", {
        { type = "combo", id = "old_name", label = "Current session", value = selected, options = names },
        { type = "text", id = "new_name", label = "New name", value = selected, hint = "new-session-name" },
    })

    if response == nil then
        return
    end

    if rename_session(response.old_name or "", response.new_name or "") then
        refresh_sessions(true)
    end
end

local function prompt_delete_session(default_name)
    refresh_sessions(true)
    local selected = default_name or pick_session("Delete Tmux Session")
    selected = trim(selected or "")
    if selected == "" then
        return
    end

    local confirmed = ui.confirm("Delete tmux session '" .. selected .. "'?")
    if not confirmed then
        return
    end

    if delete_session(selected) then
        refresh_sessions(true)
    end
end

local function render_html()
    state.action_targets = {}

    local rows = {}
    for idx, s in ipairs(state.sessions or {}) do
        local id = tostring(idx)
        state.action_targets[id] = s.name
        local row_class = (state.current_session ~= nil and s.name == state.current_session) and "tmx-row is-current" or "tmx-row"
        rows[#rows + 1] = [[
          <div class="]] .. row_class .. [[">
            <div class="tmx-name-wrap">
              <span class="tmx-current-dot"></span>
              <span class="tmx-name">]] .. html_escape(s.name) .. [[</span>
            </div>
            <div class="tmx-actions">
              <button class="tmx-icon-btn" data-action="attach:]] .. id .. [[" title="Attach" aria-label="Attach session">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 6.75A1.75 1.75 0 0 1 6.75 5h10.5A1.75 1.75 0 0 1 19 6.75v5.5A1.75 1.75 0 0 1 17.25 14H13v2.25H16l-4 4-4-4h3V14H6.75A1.75 1.75 0 0 1 5 12.25z"></path></svg>
              </button>
              <button class="tmx-icon-btn" data-action="rename:]] .. id .. [[" title="Rename" aria-label="Rename session">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m16.862 4.487 2.651 2.651a1.75 1.75 0 0 1 0 2.475l-8.82 8.82L6 19l.567-4.693 8.82-8.82a1.75 1.75 0 0 1 2.475 0M4 21h16v-2H4z"></path></svg>
              </button>
              <button class="tmx-icon-btn is-danger" data-action="delete:]] .. id .. [[" title="Delete" aria-label="Delete session">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 4h8l1 2h4v2H3V6h4zm1 6h2v8H9zm4 0h2v8h-2zM6 10h2v8H6zm10 0h2v8h-2z"></path></svg>
              </button>
            </div>
          </div>
        ]]
    end

    if #rows == 0 then
        rows[#rows + 1] = [[
          <div class="tmx-empty">
            No tmux sessions yet.
          </div>
        ]]
    end

    local error_html = ""
    if state.last_error ~= nil and state.last_error ~= "" then
        error_html = [[<div class="tmx-error">]] .. html_escape(state.last_error) .. [[</div>]]
    end

    local content = [[
      <div class="tmx-shell">
        <div class="tmx-header">
          <div class="tmx-title">Tmux Manager</div>
          <div class="tmx-header-actions">
            <button class="tmx-icon-btn" data-action="refresh" title="Refresh sessions" aria-label="Refresh sessions">
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5a7 7 0 0 1 6.93 6h-2.18l3.03 3.53L22.8 11h-1.86A9 9 0 1 0 12 21a9 9 0 0 0 8.19-5.26l-1.83-.82A7 7 0 1 1 12 5"></path></svg>
            </button>
            <button class="tmx-icon-btn" data-action="create" title="New session" aria-label="New session">
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z"></path></svg>
            </button>
          </div>
        </div>

        <div class="tmx-list">
          ]] .. table.concat(rows, "\n") .. [[
        </div>

        <div class="tmx-status">]] .. html_escape(state.status or "") .. [[</div>
        ]] .. error_html .. [[
      </div>
    ]]

    local css = [[
      .tmx-shell {
        display: flex;
        flex-direction: column;
        gap: 8px;
        color: var(--fg);
        font-size: 11px;
      }
      .tmx-header {
        display: flex;
        gap: 8px;
        align-items: center;
        justify-content: space-between;
      }
      .tmx-title {
        font-size: 13px;
        font-weight: 600;
        letter-spacing: 0.01em;
      }
      .tmx-header-actions {
        display: flex;
        gap: 5px;
      }
      .tmx-icon-btn {
        width: 24px;
        height: 24px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        border: 1px solid var(--tab-border);
        background: var(--panel-bg);
        color: var(--fg);
        border-radius: 6px;
        padding: 0;
        cursor: pointer;
        transition: background 0.12s ease, border-color 0.12s ease;
      }
      .tmx-icon-btn:hover {
        background: var(--hover-bg);
        border-color: var(--accent);
      }
      .tmx-icon-btn svg {
        width: 14px;
        height: 14px;
        fill: currentColor;
      }
      .tmx-icon-btn.is-danger:hover {
        border-color: var(--red);
        color: var(--red);
      }
      .tmx-list {
        display: flex;
        flex-direction: column;
        gap: 4px;
        border: 1px solid var(--tab-border);
        border-radius: 8px;
        background: var(--panel-bg);
        padding: 4px;
      }
      .tmx-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 3px 4px;
        border-radius: 6px;
      }
      .tmx-row.is-current {
        background: var(--hover-bg);
      }
      .tmx-name-wrap {
        min-width: 0;
        display: inline-flex;
        align-items: center;
        gap: 6px;
      }
      .tmx-current-dot {
        width: 6px;
        height: 6px;
        border-radius: 999px;
        background: transparent;
        border: 1px solid var(--tab-border);
        flex: 0 0 auto;
      }
      .tmx-row.is-current .tmx-current-dot {
        background: var(--accent);
        border-color: var(--accent);
      }
      .tmx-name {
        font-weight: 500;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .tmx-actions {
        display: flex;
        gap: 4px;
        flex: 0 0 auto;
      }
      .tmx-empty {
        color: var(--text-muted);
        text-align: center;
        padding: 14px 8px;
      }
      .tmx-status {
        color: var(--text-secondary);
        font-size: 10px;
        min-height: 14px;
      }
      .tmx-error {
        color: var(--red);
        white-space: pre-wrap;
        font-size: 10px;
      }
    ]]

    ui.panel_html(content, css)
end

local function rerender()
    render()
    ui.request_render()
end

local function handle_button_action(action_id)
    if action_id == "refresh" then
        refresh_sessions(false)
        return
    end
    if action_id == "create" then
        prompt_create_session()
        refresh_sessions(true)
        return
    end

    local verb, idx = tostring(action_id or ""):match("^([a-z_]+):(.+)$")
    if verb == nil or idx == nil then
        return
    end
    local session_name = state.action_targets[idx]
    if session_name == nil then
        return
    end

    if verb == "attach" then
        attach_session(session_name)
        return
    end
    if verb == "rename" then
        prompt_rename_session(session_name)
        return
    end
    if verb == "delete" then
        prompt_delete_session(session_name)
        return
    end
end

function setup()
    app.register_command("Tmux: Refresh Sessions", ACTION_REFRESH)
    app.register_command("Tmux: Attach Existing Session...", ACTION_ATTACH_EXISTING)
    app.register_command("Tmux: Create New Session...", ACTION_CREATE_NEW)
    app.register_command("Tmux: Rename Session...", ACTION_RENAME_EXISTING)
    app.register_command("Tmux: Delete Session...", ACTION_DELETE_EXISTING)
    app.subscribe("host.tick")
    refresh_sessions(true)
end

function render()
    local ok, err = pcall(render_html)
    if ok then
        return
    end

    ui.panel_heading("Tmux Manager")
    ui.panel_label("Render error", "error")
    ui.panel_text(tostring(err or "unknown error"))
    ui.panel_text(tostring(state.status or ""))
    if state.last_error ~= nil and state.last_error ~= "" then
        ui.panel_text(state.last_error)
    end
end

function on_event(event)
    if type(event) ~= "table" then
        return
    end

    if event.kind == "menu_action" then
        if event.action == ACTION_REFRESH then
            refresh_sessions(false)
            rerender()
            return
        end
        if event.action == ACTION_ATTACH_EXISTING then
            prompt_attach_session()
            rerender()
            return
        end
        if event.action == ACTION_CREATE_NEW then
            prompt_create_session()
            refresh_sessions(true)
            rerender()
            return
        end
        if event.action == ACTION_RENAME_EXISTING then
            prompt_rename_session(nil)
            rerender()
            return
        end
        if event.action == ACTION_DELETE_EXISTING then
            prompt_delete_session(nil)
            rerender()
            return
        end

        local dynamic_attach_target = state.attach_action_targets[event.action or ""]
        if dynamic_attach_target ~= nil then
            refresh_sessions(true, false)
            if session_by_name(dynamic_attach_target) == nil then
                local message = "Session '" .. dynamic_attach_target .. "' no longer exists."
                state.status = message
                app.notify("Tmux Manager", message, "warn", 2600)
                rerender()
                return
            end
            attach_session(dynamic_attach_target)
            rerender()
            return
        end
        return
    end

    if event.kind == "bus_event" and event.event_type == "host.tick" then
        local tick_ms = tonumber(event.data and event.data.unix_ms or 0) or 0
        local tick_unix = tick_ms > 0 and math.floor(tick_ms / 1000) or now_unix()
        poll_tmux_updates(tick_unix)
        return
    end

    if event.kind ~= "widget" or event.type ~= "button_click" then
        return
    end

    handle_button_action(event.id)
end
