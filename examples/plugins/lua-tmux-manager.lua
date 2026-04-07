-- plugin-name: Tmux Manager
-- plugin-description: Manage local tmux sessions from a docked tool window and command palette actions.
-- plugin-version: 1.8.0-diag
-- plugin-api: ^1.0
-- plugin-permissions: ui.panel, ui.menu, ui.settings, ui.notify, ui.dialog, session.exec, session.new_tab, session.rename_tab, bus.subscribe, config.read, config.write
-- plugin-type: tool_window
-- plugin-location: right
-- plugin-keybind: tmux_refresh = cmd+shift+alt+t | Refresh Tmux sessions

local ACTION_REFRESH = "tmux_refresh"
local ACTION_ATTACH_EXISTING = "tmux_attach_existing"
local ACTION_CREATE_NEW = "tmux_create_new"
local ACTION_RENAME_EXISTING = "tmux_rename_existing"
local ACTION_DELETE_EXISTING = "tmux_delete_existing"
local ACTION_RENAME_TAB_EXISTING = "tmux_rename_tab_existing"
local SETTINGS_VIEW_ID = "settings"
local SETTINGS_CONFIG_KEY = "tmux_binary_path"
local SETTINGS_PATH_LOOKUP_KEY = "tmux_use_path_lookup"

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
    tab_action_targets = {},
    tabs_by_session = {},
    expanded_sessions = {},
    context_menu = nil,
    tracked_tabs = {},
    pending_tabs_by_name = {},
    persistence_initialized = {},
    registered_attach_actions = {},
    attach_action_targets = {},
    draft_tmux_binary_path = "",
    draft_use_path_lookup = true,
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

local function get_persisted_tmux_path()
    local raw = app.get_config(SETTINGS_CONFIG_KEY)
    return trim(raw or "")
end

local function get_effective_tmux_path_for_settings()
    if type(app.get_setting_value) == "function" then
        local raw = app.get_setting_value(SETTINGS_CONFIG_KEY)
        return trim(raw or "")
    end
    return get_persisted_tmux_path()
end

local function get_effective_setting_value(key)
    if type(app.get_setting_value) == "function" then
        return app.get_setting_value(key)
    end
    return app.get_config(key)
end

local function parse_boolean_setting(value)
    if value == nil then
        return nil
    end
    local normalized = trim(tostring(value or "")):lower()
    if normalized == "true" or normalized == "\"true\"" or normalized == "1" then
        return true
    end
    if normalized == "false" or normalized == "\"false\"" or normalized == "0" then
        return false
    end
    return nil
end

local function sync_tmux_settings_draft_from_host()
    local configured = get_effective_tmux_path_for_settings()
    local lookup_raw = get_effective_setting_value(SETTINGS_PATH_LOOKUP_KEY)
    local lookup_value = parse_boolean_setting(lookup_raw)
    state.draft_tmux_binary_path = configured
    if lookup_value == nil then
        state.draft_use_path_lookup = (configured == "")
    else
        state.draft_use_path_lookup = lookup_value
    end
end

local function stage_tmux_settings_draft()
    local configured = trim(state.draft_tmux_binary_path or "")
    local next_path_value = nil
    if not state.draft_use_path_lookup and configured ~= "" then
        next_path_value = configured
    end
    local next_lookup_value = state.draft_use_path_lookup and "true" or "false"
    if type(app.set_setting_draft) == "function" then
        app.set_setting_draft(SETTINGS_CONFIG_KEY, next_path_value)
        app.set_setting_draft(SETTINGS_PATH_LOOKUP_KEY, next_lookup_value)
        return
    end
    if next_path_value == nil then
        app.set_config(SETTINGS_CONFIG_KEY, "")
    else
        app.set_config(SETTINGS_CONFIG_KEY, next_path_value)
    end
    app.set_config(SETTINGS_PATH_LOOKUP_KEY, next_lookup_value)
end

local function tmux_command_path()
    local configured = get_persisted_tmux_path()
    if configured ~= "" then
        return configured
    end
    return "tmux"
end

local function is_command_missing(result)
    local stderr = tostring((result and result.stderr) or "")
    return stderr:find("command not found", 1, true) ~= nil
        or stderr:find("No such file or directory", 1, true) ~= nil
end

local function tmux_available()
    local result = run_shell(sh_quote(tmux_command_path()) .. " -V")
    if tonumber(result.exit_code or -1) == 0 then
        return true
    end
    if is_command_missing(result) then
        return false
    end
    return false
end

-- ---------------------------------------------------------------------------
-- Diagnostic logging (sandbox-safe: uses exec_local instead of os/io)
-- Log file: ~/.config/conch/tmux-manager-diag.log
-- ---------------------------------------------------------------------------
local DIAG_LOG = nil

local function diag_log_path()
    if DIAG_LOG ~= nil then return DIAG_LOG end
    local r = session.exec_local("echo $HOME")
    local home = trim(r.stdout or "")
    if home == "" then home = "/tmp" end
    DIAG_LOG = home .. "/.config/conch/tmux-manager-diag.log"
    return DIAG_LOG
end

local function dlog(msg)
    local ts_result = session.exec_local("date '+%Y-%m-%d %H:%M:%S'")
    local ts = trim(ts_result.stdout or "?")
    local line = "[" .. ts .. "] " .. tostring(msg)
    session.exec_local("echo " .. sh_quote(line) .. " >> " .. sh_quote(diag_log_path()))
end

local function run_tmux(args)
    local cmd = sh_quote(tmux_command_path()) .. " " .. args
    local result = run_shell(cmd)
    dlog("run_tmux: " .. args .. " => exit=" .. tostring(result.exit_code)
        .. (trim(result.stderr or "") ~= "" and (" stderr=" .. trim(result.stderr)) or ""))
    return result
end

local function dlog_tmux_state(label)
    dlog("--- " .. label .. " ---")
    local sv = run_shell(sh_quote(tmux_command_path()) .. " display-message -p '#{version}' 2>&1")
    dlog("  tmux version: " .. trim(sv.stdout or "?") .. " (exit=" .. tostring(sv.exit_code) .. ")")

    local ls = run_shell(sh_quote(tmux_command_path())
        .. " list-sessions -F '#{session_name} attached=#{session_attached} windows=#{session_windows} id=#{session_id}' 2>&1")
    for _, line in ipairs(split_lines(ls.stdout or "")) do
        if trim(line) ~= "" then dlog("  session: " .. line) end
    end
    if trim(ls.stdout or "") == "" then
        dlog("  (no sessions or server not running, stderr=" .. trim(ls.stderr or "") .. ")")
    end

    local sessions = run_shell(sh_quote(tmux_command_path()) .. " list-sessions -F '#{session_name}' 2>/dev/null")
    for _, sname in ipairs(split_lines(sessions.stdout or "")) do
        sname = trim(sname)
        if sname ~= "" then
            local opts = run_shell(sh_quote(tmux_command_path())
                .. " show-options -t " .. sh_quote(sname) .. " 2>/dev/null")
            dlog("  options[" .. sname .. "]: " .. trim(opts.stdout or "(none)"):gsub("\n", " | "))

            local wopts = run_shell(sh_quote(tmux_command_path())
                .. " show-window-options -t " .. sh_quote(sname) .. " 2>/dev/null")
            dlog("  wopts[" .. sname .. "]:   " .. trim(wopts.stdout or "(none)"):gsub("\n", " | "))

            local panes = run_shell(sh_quote(tmux_command_path())
                .. " list-panes -t " .. sh_quote(sname)
                .. " -s -F '#{pane_id} pid=#{pane_pid} dead=#{pane_dead} cmd=#{pane_current_command}' 2>/dev/null")
            for _, pline in ipairs(split_lines(panes.stdout or "")) do
                if trim(pline) ~= "" then dlog("  pane[" .. sname .. "]: " .. pline) end
            end
        end
    end

    local gopts = run_shell(sh_quote(tmux_command_path()) .. " show-window-options -g 2>/dev/null")
    local remain_line = ""
    for _, line in ipairs(split_lines(gopts.stdout or "")) do
        if line:find("remain-on-exit", 1, true) then remain_line = trim(line) end
    end
    dlog("  global wopts: " .. (remain_line ~= "" and remain_line or "remain-on-exit (default)"))

    local global_sopts = run_shell(sh_quote(tmux_command_path()) .. " show-options -g 2>/dev/null")
    local destroy_line = ""
    for _, line in ipairs(split_lines(global_sopts.stdout or "")) do
        if line:find("destroy-unattached", 1, true) then destroy_line = trim(line) end
    end
    if destroy_line ~= "" then dlog("  global sopts: " .. destroy_line) end

    local server_opts = run_shell(sh_quote(tmux_command_path()) .. " show-options -s 2>/dev/null")
    dlog("  server opts: " .. trim(server_opts.stdout or "(none)"):gsub("\n", " | "))
    dlog("--- end " .. label .. " ---")
end

-- Respawn any dead panes in a managed session so the user never sees
-- "[Pane is dead]" after an app restart.
local function respawn_dead_panes(session_name)
    local result = run_tmux("list-panes -t " .. sh_quote(session_name)
        .. " -s -F '#{pane_id} #{pane_dead}'")
    if (result.exit_code or -1) ~= 0 then return 0 end
    local count = 0
    for _, line in ipairs(split_lines(result.stdout or "")) do
        local pane_id, dead = line:match("^(%S+)%s+(%d+)")
        if pane_id and dead == "1" then
            dlog("respawning dead pane " .. pane_id .. " in session " .. session_name)
            run_tmux("respawn-pane -k -t " .. sh_quote(pane_id))
            count = count + 1
        end
    end
    if count > 0 then
        dlog("respawned " .. count .. " dead pane(s) in " .. session_name)
    end
    return count
end
-- ---------------------------------------------------------------------------

local function session_by_name(name)
    for _, s in ipairs(state.sessions or {}) do
        if s.name == name then
            return s
        end
    end
    return nil
end

local function session_by_id(id)
    local target = tostring(id or "")
    if target == "" then
        return nil
    end
    for _, s in ipairs(state.sessions or {}) do
        if tostring(s.id or "") == target then
            return s
        end
    end
    return nil
end

local function session_key(session_info)
    if session_info == nil then
        return ""
    end
    local sid = tostring(session_info.id or "")
    if sid ~= "" then
        return sid
    end
    return tostring(session_info.name or "")
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
    local cmd = "env -u TMUX " .. sh_quote(tmux_command_path()) .. " " .. args .. "\n"
    state.status = "Launching: " .. cmd:gsub("\n$", "")
    return session.new_tab_with_title(cmd, true, tab_title)
end

local function tracked_tab_id_for_session(name)
    local known = session_by_name(name)
    if known ~= nil and tonumber(known.created_unix or 0) > 0 then
        local tracked = state.tracked_tabs[tostring(known.created_unix)]
        if tracked ~= nil and tracked.tab_id ~= nil and tracked.tab_id ~= "" then
            return trim(tostring(tracked.tab_id))
        end
    end

    local pending = state.pending_tabs_by_name[name]
    if pending ~= nil and pending ~= "" then
        return trim(tostring(pending))
    end
    return nil
end

local function ensure_session_persistence(name, opts)
    name = trim(name)
    if name == "" then
        return
    end
    opts = opts or {}
    dlog("ensure_session_persistence: name=" .. name .. " opts.force=" .. tostring(opts.force)
        .. " opts.refresh_windows=" .. tostring(opts.refresh_windows))

    local state_for_session = state.persistence_initialized[name]
    if state_for_session == nil then
        state_for_session = { base = false, windows = false }
        state.persistence_initialized[name] = state_for_session
    end

    local force = opts.force == true
    local refresh_windows = opts.refresh_windows == true
    if state_for_session.base == true and not force and not refresh_windows then
        dlog("  persistence already initialized, skipping")
        return
    end

    -- Keep session state alive when client tabs are closed/reopened and
    -- avoid losing a window if the attached pane exits unexpectedly.
    if force or state_for_session.base ~= true then
        dlog("  setting base persistence options")
        run_tmux("set-option -q -t " .. sh_quote(name) .. " destroy-unattached off")
        run_tmux("set-option -q -t " .. sh_quote(name) .. " detach-on-destroy off")
        run_tmux("set-window-option -q -t " .. sh_quote(name) .. " remain-on-exit on")

        -- Auto-respawn dead panes at the tmux-server level so the session
        -- stays alive even after Conch is closed.  The hook fires inside the
        -- tmux server and does not require Conch to be running.
        run_tmux("set-hook -t " .. sh_quote(name) .. " " .. sh_quote("pane-died[99]") .. " " .. sh_quote("respawn-pane -k"))
        dlog("  installed per-session pane-died auto-respawn hook")

        state_for_session.base = true
    end

    if not refresh_windows and not force then
        return
    end

    if force or state_for_session.windows ~= true then
        dlog("  setting per-window remain-on-exit")
        local windows_result = run_tmux("list-windows -t " .. sh_quote(name) .. " -F '#{window_id}'")
        if tonumber(windows_result.exit_code or -1) ~= 0 then
            dlog("  list-windows failed, aborting per-window setup")
            return
        end
        for _, line in ipairs(split_lines(windows_result.stdout or "")) do
            local wid = trim(line)
            if wid ~= "" then
                run_tmux("set-window-option -q -t " .. sh_quote(wid) .. " remain-on-exit on")
            end
        end
        state_for_session.windows = true
    end
end

local function tabs_for_session(session_info)
    local key = session_key(session_info)
    if key == "" then
        return {}
    end
    return state.tabs_by_session[key] or {}
end

local function is_session_expanded(session_info)
    local key = session_key(session_info)
    if key == "" then
        return false
    end
    return state.expanded_sessions[key] == true
end

local function first_expanded_session()
    for key, expanded in pairs(state.expanded_sessions or {}) do
        if expanded == true then
            local session_info = session_by_id(key) or session_by_name(key)
            if session_info ~= nil then
                return session_info
            end
        end
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

local function list_windows_for_session(session_name)
    local cmd = "list-windows -t "
        .. sh_quote(session_name)
        .. " -F '#{window_id}\t#{window_index}\t#{window_name}\t#{window_active}\t#{window_panes}'"
    local result = run_tmux(cmd)
    if tonumber(result.exit_code or -1) ~= 0 then
        if is_no_server(result) then
            return {}
        end
        local err = tostring(result.stderr or "")
        if err:find("can't find session", 1, true) ~= nil then
            return {}
        end
        return nil, trim(err ~= "" and err or "Unable to list tmux windows.")
    end

    local tabs = {}
    for _, line in ipairs(split_lines(result.stdout or "")) do
        if trim(line) ~= "" then
            local fields = split_tab(line)
            tabs[#tabs + 1] = {
                id = fields[1] or "",
                index = tonumber(fields[2] or "0") or 0,
                name = fields[3] or "",
                active = tostring(fields[4] or "0") == "1",
                panes = tonumber(fields[5] or "0") or 0,
            }
        end
    end

    table.sort(tabs, function(a, b)
        if tonumber(a.index or 0) == tonumber(b.index or 0) then
            return tostring(a.name or ""):lower() < tostring(b.name or ""):lower()
        end
        return tonumber(a.index or 0) < tonumber(b.index or 0)
    end)
    return tabs
end

local function refresh_session_tabs(parsed_sessions)
    local next_tabs = {}
    local tab_error = nil
    for _, s in ipairs(parsed_sessions or {}) do
        local key = session_key(s)
        if key ~= "" and state.expanded_sessions[key] == true then
            local tabs, err = list_windows_for_session(s.name or "")
            if tabs == nil then
                tabs = {}
                if tab_error == nil then
                    tab_error = err
                end
            end
            next_tabs[key] = tabs
        end
    end
    state.tabs_by_session = next_tabs
    if tab_error ~= nil and tab_error ~= "" then
        state.last_error = tab_error
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
        state.persistence_initialized = {}
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

    local list = run_tmux("list-sessions -F '#{session_id}\t#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}'")
    if tonumber(list.exit_code or -1) ~= 0 then
        if is_no_server(list) then
            state.sessions = {}
            state.persistence_initialized = {}
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
            local attached_clients = tonumber(fields[4] or "0") or 0
            parsed[#parsed + 1] = {
                id = fields[1] or "",
                name = fields[2] or "",
                windows = tonumber(fields[3] or "0") or 0,
                attached_clients = attached_clients,
                created_unix = tonumber(fields[5] or "0") or 0,
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
    local live_session_names = {}
    for _, s in ipairs(parsed or {}) do
        live_session_names[s.name or ""] = true
    end
    for known_name, _ in pairs(state.persistence_initialized or {}) do
        if not live_session_names[known_name] then
            state.persistence_initialized[known_name] = nil
        end
    end

    local seen_expanded = {}
    for _, s in ipairs(parsed or {}) do
        local key = session_key(s)
        if key ~= "" then
            seen_expanded[key] = true
        end
    end
    for key, expanded in pairs(state.expanded_sessions or {}) do
        if expanded == true and not seen_expanded[key] then
            state.expanded_sessions[key] = nil
        end
    end

    refresh_session_tabs(parsed)

    if state.context_menu ~= nil then
        state.context_menu = nil
    end

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
            s.id or "",
            s.name or "",
            tostring(s.windows or 0),
            tostring(s.attached_clients or 0),
            tostring(s.created_unix or 0),
        }, "|")
        for _, tab in ipairs(tabs_for_session(s)) do
            parts[#parts + 1] = table.concat({
                tab.id or "",
                tostring(tab.index or 0),
                tab.name or "",
                tab.active and "1" or "0",
                tostring(tab.panes or 0),
            }, "|")
        end
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
    state.next_poll_unix = now_value + 3

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

    dlog("attach_session: name=" .. name)
    dlog_tmux_state("before attach " .. name)

    ensure_session_persistence(name, { refresh_windows = true })

    -- Respawn any dead panes left from a previous app exit so the user
    -- never sees "[Pane is dead]" when re-attaching.
    respawn_dead_panes(name)

    local existing_tab_id = tracked_tab_id_for_session(name)
    if existing_tab_id ~= nil then
        dlog("  focusing existing tab " .. existing_tab_id)
        session.focus_tab_by_id(existing_tab_id)
        state.status = "Switched to existing tab for tmux session '" .. name .. "'."
        return
    end

    local tab_id = launch_tmux_in_plain_tab(
        "attach-session -t " .. sh_quote(name),
        name
    )
    dlog("  launched tab, tab_id=" .. tostring(tab_id))
    dlog_tmux_state("after attach launched " .. name)
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

local function attach_session_tab(name, window_id, _window_index, window_label)
    name = trim(name)
    window_id = trim(window_id)
    if name == "" or window_id == "" then
        app.notify("Tmux Manager", "Session and tab are required.", "warn", 2400)
        return
    end

    ensure_session_persistence(name)

    local tab_title = name

    local existing_tab_id = tracked_tab_id_for_session(name)
    if existing_tab_id ~= nil then
        session.focus_tab_by_id(existing_tab_id)
        local switch_result = run_tmux("select-window -t " .. sh_quote(window_id))
        if tonumber(switch_result.exit_code or -1) ~= 0 then
            local message = trim(switch_result.stderr or "Unable to switch tmux tab.")
            state.status = "Focused existing tab for '" .. name .. "', but switch failed."
            state.last_error = message
            app.notify("Tmux Manager", message, "warn", 2600)
            return
        end
        state.status = "Switched existing tab for '" .. name .. "' to '" .. tostring(window_label or "") .. "'."
        return
    end

    local attach_cmd = "attach-session -t " .. sh_quote(name) .. " \\; select-window -t " .. sh_quote(window_id)

    local tab_id = launch_tmux_in_plain_tab(attach_cmd, tab_title)
    if tab_id ~= nil and tab_id ~= "" then
        local known = session_by_name(name)
        if known ~= nil and tonumber(known.created_unix or 0) > 0 then
            state.tracked_tabs[tostring(known.created_unix)] = { tab_id = tab_id, last_name = name }
        else
            state.pending_tabs_by_name[name] = tab_id
        end
    end

    state.status = "Opening tmux tab '" .. tostring(window_label or "") .. "' from '" .. name .. "'..."
end

local function create_session(name)
    name = trim(name)
    if name == "" then
        app.notify("Tmux Manager", "Session name cannot be empty.", "warn", 2600)
        return
    end

    dlog("create_session: name=" .. name)
    dlog_tmux_state("before create " .. name)

    local create_result = run_tmux("new-session -d -s " .. sh_quote(name))
    if tonumber(create_result.exit_code or -1) ~= 0 then
        local message = trim(create_result.stderr or "Unable to create tmux session.")
        state.status = "Create failed."
        state.last_error = message
        dlog("  create FAILED: " .. message)
        app.notify("Tmux Manager", message, "error", 3800)
        return
    end

    dlog_tmux_state("after new-session " .. name)

    ensure_session_persistence(name, { refresh_windows = true })

    dlog_tmux_state("after persistence " .. name)

    local tab_id = launch_tmux_in_plain_tab("attach-session -t " .. sh_quote(name), name)
    dlog("  launched tab, tab_id=" .. tostring(tab_id))
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
    local persist_state = state.persistence_initialized[old_name]
    if persist_state ~= nil then
        state.persistence_initialized[old_name] = nil
        state.persistence_initialized[new_name] = persist_state
    end
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
    state.persistence_initialized[name] = nil
    app.notify("Tmux Manager", state.status, "success", 2200)
    return true
end

local function toggle_session_expanded(session_ref)
    local found = nil
    if type(session_ref) == "table" then
        found = session_by_id(session_ref.id or "") or session_by_name(session_ref.name or "")
    else
        found = session_by_id(session_ref or "") or session_by_name(session_ref or "")
    end
    if found == nil then
        state.expanded_sessions = {}
        state.context_menu = nil
        state.status = "Session no longer exists."
        return false
    end

    local key = session_key(found)
    if key == "" then
        return false
    end

    if state.expanded_sessions[key] == true then
        state.expanded_sessions[key] = nil
        state.context_menu = nil
        state.tabs_by_session[key] = nil
        state.status = "Collapsed '" .. tostring(found.name or "") .. "'."
        return true
    end

    state.expanded_sessions[key] = true
    state.context_menu = nil
    refresh_session_tabs(state.sessions)
    state.status = "Expanded '" .. tostring(found.name or "") .. "'."
    return true
end

local function rename_tmux_tab(window_id, new_name)
    window_id = trim(window_id or "")
    new_name = trim(new_name or "")

    if window_id == "" or new_name == "" then
        app.notify("Tmux Manager", "Tab id and name are required.", "warn", 2600)
        return false
    end

    local result = run_tmux("rename-window -t " .. sh_quote(window_id) .. " " .. sh_quote(new_name))
    if tonumber(result.exit_code or -1) ~= 0 then
        local message = trim(result.stderr or "Unable to rename tmux tab.")
        state.status = "Rename tab failed."
        state.last_error = message
        app.notify("Tmux Manager", message, "error", 3800)
        return false
    end

    state.status = "Renamed tmux tab to '" .. new_name .. "'."
    return true
end

local function prompt_rename_tab_for_session(session_info, default_tab)
    if session_info == nil then
        app.notify("Tmux Manager", "Session is required.", "warn", 2400)
        return
    end

    local tabs = tabs_for_session(session_info)
    if #tabs == 0 then
        local live_tabs = list_windows_for_session(session_info.name or "")
        if type(live_tabs) == "table" then
            tabs = live_tabs
        end
        if #tabs == 0 then
            app.notify("Tmux Manager", "No tabs found for this session.", "warn", 2600)
            return
        end
    end

    local selected_tab = default_tab
    if selected_tab == nil then
        selected_tab = tabs[1]
    end
    if selected_tab == nil then
        return
    end

    local response = ui.form("Rename Tmux Tab", {
        {
            type = "text",
            id = "new_name",
            label = "New tab name",
            value = tostring(selected_tab.name or ""),
            hint = "new-tab-name",
        },
    })
    if response == nil then
        return
    end

    if rename_tmux_tab(selected_tab.id or "", response.new_name or "") then
        refresh_sessions(true)
    end
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

local function prompt_rename_tab()
    refresh_sessions(true)

    local chosen_session = first_expanded_session()
    if chosen_session == nil then
        local session_name = pick_session("Choose Session For Tab Rename")
        if session_name == nil or session_name == "" then
            return
        end
        chosen_session = session_by_name(session_name)
    end
    if chosen_session == nil then
        app.notify("Tmux Manager", "Session not found.", "warn", 2400)
        return
    end

    local tabs = tabs_for_session(chosen_session)
    if #tabs == 0 then
        local live_tabs = list_windows_for_session(chosen_session.name or "")
        if type(live_tabs) == "table" then
            tabs = live_tabs
        end
    end
    if #tabs == 0 then
        app.notify("Tmux Manager", "No tabs found for this session.", "warn", 2600)
        return
    end

    local labels = {}
    local by_label = {}
    for _, tab in ipairs(tabs) do
        local label = tostring(tab.index or 0) .. ": " .. tostring(tab.name or "")
        labels[#labels + 1] = label
        by_label[label] = tab
    end

    local response = ui.form("Rename Tmux Tab", {
        { type = "combo", id = "tab_choice", label = "Tab", value = labels[1], options = labels },
        {
            type = "text",
            id = "new_name",
            label = "New tab name",
            value = tostring(tabs[1].name or ""),
            hint = "new-tab-name",
        },
    })
    if response == nil then
        return
    end

    local target = by_label[tostring(response.tab_choice or "")]
    if target == nil then
        return
    end

    if rename_tmux_tab(target.id or "", response.new_name or "") then
        refresh_sessions(true)
    end
end

local function render_html()
    state.action_targets = {}
    state.tab_action_targets = {}

    local rows = {}
    local header_title = "Tmux Manager"
    local header_subtitle = "Sessions"
    local header_actions = [[
      <button class="tmx-icon-btn" data-action="refresh" title="Refresh sessions" aria-label="Refresh sessions">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5a7 7 0 0 1 6.93 6h-2.18l3.03 3.53L22.8 11h-1.86A9 9 0 1 0 12 21a9 9 0 0 0 8.19-5.26l-1.83-.82A7 7 0 1 1 12 5"></path></svg>
      </button>
      <button class="tmx-icon-btn" data-action="create" title="New session" aria-label="New session">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z"></path></svg>
      </button>
    ]]

    for idx, s in ipairs(state.sessions or {}) do
        local id = tostring(idx)
        state.action_targets[id] = s
        local expanded = is_session_expanded(s)
        local row_class = "tmx-row"
        local session_tab_id = tracked_tab_id_for_session(s.name or "") or ""
        local chevron = expanded and "&#9662;" or "&#9656;"
        local tabs = expanded and tabs_for_session(s) or {}
        local tab_rows = {}

        for t_idx, tab in ipairs(tabs) do
            local tab_id = id .. ":" .. tostring(t_idx)
            state.tab_action_targets[tab_id] = { session = s, tab = tab }
            local tab_row_class = tab.active and "tmx-tab-row is-current" or "tmx-tab-row"
            tab_rows[#tab_rows + 1] = [[
              <div class="tmx-tab-wrap">
                <button class="]] .. tab_row_class .. [[" data-action="open_tab:]] .. tab_id .. [[" data-context-action="show_tab_menu:]] .. tab_id .. [[" title="Click to open this tmux tab. Right-click for actions.">
                  <span class="tmx-name-wrap">
                    <span class="tmx-tab-index">]] .. tostring(tab.index or 0) .. [[</span>
                    <span class="tmx-name">]] .. html_escape(tab.name) .. [[</span>
                  </span>
                  <span class="tmx-meta">]] .. tostring(tab.panes or 0) .. [[ pane(s)</span>
                </button>
              </div>
            ]]
        end

        if #tab_rows == 0 then
            tab_rows[#tab_rows + 1] = [[<div class="tmx-empty tmx-empty-tabs">No tabs in this session.</div>]]
        end

        rows[#rows + 1] = [[
          <div class="tmx-session">
            <div class="]] .. row_class .. [[" data-session-tab-id="]] .. html_escape(session_tab_id) .. [[">
              <button class="tmx-row-main" data-action="toggle_session:]] .. id .. [[" data-context-action="show_session_menu:]] .. id .. [[" title="Click to expand tabs. Right-click for actions.">
                <span class="tmx-name-wrap">
                  <span class="tmx-chevron">]] .. chevron .. [[</span>
                  <span class="tmx-current-dot"></span>
                  <span class="tmx-name">]] .. html_escape(s.name) .. [[</span>
                </span>
                <span class="tmx-meta">]] .. tostring(s.windows or 0) .. [[ tab(s)</span>
              </button>
              <div class="tmx-actions">
                <button class="tmx-icon-btn" data-action="attach_session:]] .. id .. [[" title="Attach" aria-label="Attach session">
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 6.75A1.75 1.75 0 0 1 6.75 5h10.5A1.75 1.75 0 0 1 19 6.75v5.5A1.75 1.75 0 0 1 17.25 14H13v2.25H16l-4 4-4-4h3V14H6.75A1.75 1.75 0 0 1 5 12.25z"></path></svg>
                </button>
                <button class="tmx-icon-btn is-danger" data-action="delete_session:]] .. id .. [[" title="Delete" aria-label="Delete session">
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 4h8l1 2h4v2H3V6h4zm1 6h2v8H9zm4 0h2v8h-2zM6 10h2v8H6zm10 0h2v8h-2z"></path></svg>
                </button>
              </div>
            </div>
            <div class="tmx-tabs-wrap ]] .. (expanded and "is-open" or "") .. [[">
              <div class="tmx-tabs-shell">
                <div class="tmx-tabs-hint">Click a tab to open it. Right-click for tab actions.</div>
                ]] .. table.concat(tab_rows, "\n") .. [[
              </div>
            </div>
          </div>
        ]]
    end

    local context_menu_html = ""
    if state.context_menu ~= nil then
        local menu_items = {}
        local target = tostring(state.context_menu.target or "")
        if state.context_menu.kind == "session" then
            if state.action_targets[target] ~= nil then
                menu_items[#menu_items + 1] = [[<button class="tmx-context-item" data-action="rename_session:]] .. target .. [[">Rename Session</button>]]
            end
        elseif state.context_menu.kind == "tab" then
            if state.tab_action_targets[target] ~= nil then
                menu_items[#menu_items + 1] = [[<button class="tmx-context-item" data-action="rename_tab:]] .. target .. [[">Rename Tab</button>]]
            end
        end
        if #menu_items > 0 then
            local x = math.floor(tonumber(state.context_menu.x or 0) or 0)
            local y = math.floor(tonumber(state.context_menu.y or 0) or 0)
            context_menu_html = [[
              <button class="tmx-context-backdrop" data-action="close_menu" aria-label="Close menu"></button>
              <div class="tmx-context-menu" style="left: ]] .. tostring(x) .. [[px; top: ]] .. tostring(y) .. [[px;">
                ]] .. table.concat(menu_items, "\n") .. [[
              </div>
            ]]
        end
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
          <div class="tmx-title-wrap">
            <div class="tmx-title">]] .. html_escape(header_title) .. [[</div>
            <div class="tmx-subtitle">]] .. html_escape(header_subtitle) .. [[</div>
          </div>
          <div class="tmx-header-actions">]] .. header_actions .. [[</div>
        </div>

        <div class="tmx-list">
          ]] .. table.concat(rows, "\n") .. [[
        </div>
        ]] .. context_menu_html .. [[

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
      .tmx-title-wrap {
        min-width: 0;
        display: flex;
        flex-direction: column;
      }
      .tmx-title {
        font-size: 13px;
        font-weight: 600;
        letter-spacing: 0.01em;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .tmx-subtitle {
        color: var(--text-secondary);
        font-size: 10px;
        min-height: 12px;
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
        gap: 6px;
        border: 1px solid var(--tab-border);
        border-radius: 8px;
        background: var(--panel-bg);
        padding: 4px;
      }
      .tmx-session {
        display: flex;
        flex-direction: column;
        gap: 4px;
      }
      .tmx-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 2px;
        border-radius: 6px;
      }
      .tmx-row.is-current {
        background: var(--hover-bg);
      }
      .tmx-row-main {
        border: none;
        background: transparent;
        color: inherit;
        width: 100%;
        min-width: 0;
        display: inline-flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 3px 4px;
        border-radius: 6px;
        text-align: left;
        cursor: pointer;
      }
      .tmx-row-main:hover {
        background: var(--hover-bg);
      }
      .tmx-chevron {
        width: 10px;
        text-align: center;
        color: var(--text-secondary);
        font-size: 10px;
      }
      .tmx-name-wrap {
        min-width: 0;
        display: inline-flex;
        align-items: center;
        gap: 6px;
      }
      .tmx-tab-index {
        font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
        font-size: 10px;
        color: var(--text-secondary);
        border: 1px solid var(--tab-border);
        border-radius: 4px;
        padding: 0 4px;
        line-height: 14px;
      }
      .tmx-current-dot {
        width: 6px;
        height: 6px;
        border-radius: 999px;
        background: transparent;
        opacity: 0;
        flex: 0 0 auto;
      }
      .tmx-row.is-current .tmx-current-dot {
        background: var(--green);
        opacity: 1;
      }
      .tmx-name {
        font-weight: 500;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      .tmx-meta {
        color: var(--text-secondary);
        font-size: 10px;
        flex: 0 0 auto;
      }
      .tmx-actions {
        display: flex;
        gap: 4px;
        flex: 0 0 auto;
      }
      .tmx-tabs-wrap {
        overflow: hidden;
        max-height: 0;
        opacity: 0;
        border-left-color: transparent;
        transition:
          max-height 180ms cubic-bezier(0.2, 0.8, 0.2, 1),
          opacity 140ms ease,
          border-left-color 140ms ease;
        margin-left: 14px;
        border-left: 1px solid var(--tab-border);
        will-change: max-height, opacity;
      }
      .tmx-tabs-wrap.is-open {
        max-height: var(--tmx-open-height, 360px);
        opacity: 1;
        border-left-color: var(--tab-border);
      }
      .tmx-tabs-shell {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 4px 0 4px 8px;
      }
      .tmx-tabs-hint {
        color: var(--text-secondary);
        font-size: 10px;
        margin-bottom: 2px;
      }
      .tmx-tab-wrap {
        display: flex;
        flex-direction: column;
        gap: 3px;
      }
      .tmx-tab-row {
        border: none;
        background: transparent;
        color: inherit;
        width: 100%;
        min-width: 0;
        display: inline-flex;
        align-items: center;
        justify-content: space-between;
        gap: 6px;
        padding: 3px 4px;
        border-radius: 6px;
        text-align: left;
        cursor: pointer;
      }
      .tmx-tab-row:hover {
        background: var(--hover-bg);
      }
      .tmx-tab-row.is-current {
        background: var(--hover-bg);
      }
      .tmx-context-backdrop {
        position: fixed;
        inset: 0;
        background: transparent;
        border: none;
        margin: 0;
        padding: 0;
        z-index: 3997;
      }
      .tmx-context-menu {
        position: fixed;
        min-width: 148px;
        background: var(--panel-bg);
        border: 1px solid var(--tab-border);
        border-radius: 8px;
        box-shadow: 0 10px 24px rgba(0, 0, 0, 0.38);
        overflow: hidden;
        z-index: 3998;
        animation: tmx-menu-in 0.11s ease;
      }
      @keyframes tmx-menu-in {
        from { opacity: 0; transform: translateY(-3px) scale(0.98); }
        to { opacity: 1; transform: translateY(0) scale(1); }
      }
      .tmx-context-item {
        display: block;
        width: 100%;
        border: none;
        border-bottom: 1px solid var(--tab-border);
        background: transparent;
        color: var(--fg);
        text-align: left;
        padding: 7px 10px;
        font-size: 11px;
        cursor: pointer;
      }
      .tmx-context-item:last-child {
        border-bottom: none;
      }
      .tmx-context-item:hover {
        background: var(--hover-bg);
      }
      .tmx-empty {
        color: var(--text-muted);
        text-align: center;
        padding: 14px 8px;
      }
      .tmx-empty-tabs {
        text-align: left;
        padding: 6px 8px;
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

local function render_settings_view()
    sync_tmux_settings_draft_from_host()

    ui.panel_vertical(function()
        ui.panel_label("Tmux Binary Path", "normal")
        ui.panel_label(
            "Optional explicit path to the tmux executable.",
            "secondary"
        )
        ui.panel_text_input(
            "tmux_binary_path_input",
            state.draft_tmux_binary_path or "",
            "Leave blank to use 'tmux' from PATH",
            not state.draft_use_path_lookup
        )
        ui.panel_horizontal(function()
            ui.panel_vertical(function()
                ui.panel_label("Use PATH lookup", "normal")
                ui.panel_label(
                    "Use 'tmux' from PATH instead of a custom binary path.",
                    "secondary"
                )
            end, 2)
            ui.panel_spacer()
            ui.panel_checkbox(
                "tmux_use_path_lookup_toggle",
                "",
                state.draft_use_path_lookup
            )
        end, 10)
    end, 8)
end

local function handle_settings_event(event)
    if event.type == "text_input_changed" and event.id == "tmux_binary_path_input" then
        state.draft_tmux_binary_path = trim(event.value or "")
        stage_tmux_settings_draft()
        return true
    end
    if event.type == "text_input_submit" and event.id == "tmux_binary_path_input" then
        state.draft_tmux_binary_path = trim(event.value or "")
        stage_tmux_settings_draft()
        return true
    end
    if event.type == "checkbox_changed" and event.id == "tmux_use_path_lookup_toggle" then
        state.draft_use_path_lookup = event.checked == true
        stage_tmux_settings_draft()
        return true
    end
    return false
end

local function rerender()
    render()
    ui.request_render()
end

local function handle_button_action(action_id, context_x, context_y)
    state.context_menu = nil

    if action_id == "refresh" then
        refresh_sessions(false)
        return
    end
    if action_id == "create" then
        prompt_create_session()
        refresh_sessions(true)
        return
    end
    if action_id == "close_menu" then
        state.context_menu = nil
        return
    end

    local verb, idx = tostring(action_id or ""):match("^([a-z_]+):(.+)$")
    if verb == nil or idx == nil then
        return
    end

    if verb == "show_session_menu" then
        state.context_menu = {
            kind = "session",
            target = idx,
            x = math.floor(tonumber(context_x or 24) or 24),
            y = math.floor(tonumber(context_y or 24) or 24),
        }
        return
    end

    if verb == "show_tab_menu" then
        state.context_menu = {
            kind = "tab",
            target = idx,
            x = math.floor(tonumber(context_x or 24) or 24),
            y = math.floor(tonumber(context_y or 24) or 24),
        }
        return
    end

    if verb == "open_tab" or verb == "rename_tab" then
        local target = state.tab_action_targets[idx]
        if target == nil or target.session == nil or target.tab == nil then
            return
        end
        if verb == "open_tab" then
            attach_session_tab(
                target.session.name or "",
                target.tab.id or "",
                target.tab.index or -1,
                target.tab.name or ""
            )
            return
        end
        prompt_rename_tab_for_session(target.session, target.tab)
        return
    end

    local target = state.action_targets[idx]
    if target == nil then
        return
    end

    if verb == "toggle_session" then
        toggle_session_expanded(target)
        return
    end
    if verb == "attach_session" then
        attach_session(target.name or "")
        return
    end
    if verb == "rename_session" then
        prompt_rename_session(target.name or "")
        return
    end
    if verb == "delete_session" then
        prompt_delete_session(target.name or "")
        return
    end
end

function setup()
    dlog("===== PLUGIN SETUP START =====")
    dlog_tmux_state("setup initial state")

    -- Clean up stale global remain-on-exit from previous plugin versions.
    -- Older versions set this globally (-g) which polluted all tmux sessions.
    -- Now we set it per-window on managed sessions only.
    run_tmux("set-window-option -g -uq remain-on-exit 2>/dev/null")
    dlog("cleared stale global remain-on-exit")

    -- Respawn any dead panes across all sessions left from a previous exit.
    local ls = run_tmux("list-sessions -F '#{session_name}' 2>/dev/null")
    for _, sname in ipairs(split_lines(ls.stdout or "")) do
        sname = trim(sname)
        if sname ~= "" then
            respawn_dead_panes(sname)
        end
    end

    -- Install tmux hooks to log session lifecycle events to the diag log.
    -- These fire inside the tmux server regardless of whether Conch is running.
    local log_path = diag_log_path()
    local hooks = {
        { "client-detached",         "client-detached session=#{session_name} client=#{client_name}" },
        { "client-session-changed",  "client-session-changed session=#{session_name}" },
        { "session-closed",          "session-closed session=#{session_name}" },
        { "window-linked",           "window-linked session=#{session_name} window=#{window_name}" },
        { "window-unlinked",         "window-unlinked session=#{session_name} window=#{window_name}" },
        { "pane-died",               "pane-died session=#{session_name} pane=#{pane_id} pid=#{pane_pid} status=#{pane_dead_status} signal=#{pane_dead_signal}" },
        { "pane-exited",             "pane-exited session=#{session_name} pane=#{pane_id} pid=#{pane_pid}" },
    }
    for _, h in ipairs(hooks) do
        -- Build: tmux set-hook -g <hook> 'run-shell "echo ... >> logfile"'
        local inner = "echo \\\"[$(date +%%Y-%%m-%%d\\ %%H:%%M:%%S)] tmux-hook: " .. h[2] .. "\\\" >> " .. log_path:gsub('"', '\\"')
        run_shell(sh_quote(tmux_command_path())
            .. " set-hook -g " .. h[1]
            .. " 'run-shell \"" .. inner .. "\"'"
            .. " 2>/dev/null")
    end
    dlog("tmux hooks installed")

    sync_tmux_settings_draft_from_host()
    app.register_settings_section({
        id = "tmux-manager",
        label = "Tmux Manager",
        description = "Configure Tmux Manager behavior.",
        keywords = "tmux manager sessions binary path",
        group = "Extensions",
        view_id = SETTINGS_VIEW_ID,
        settings = {
            {
                id = "tmux_binary_path_input",
                label = "Tmux Binary Path",
                description = "Optional explicit path to the tmux executable.",
                keywords = "tmux binary path executable command",
            },
        },
    })
    app.register_command("Tmux: Refresh Sessions", ACTION_REFRESH)
    app.register_command("Tmux: Attach Existing Session...", ACTION_ATTACH_EXISTING)
    app.register_command("Tmux: Create New Session...", ACTION_CREATE_NEW)
    app.register_command("Tmux: Rename Session...", ACTION_RENAME_EXISTING)
    app.register_command("Tmux: Delete Session...", ACTION_DELETE_EXISTING)
    app.register_command("Tmux: Rename Tab...", ACTION_RENAME_TAB_EXISTING)
    app.subscribe("host.tick")
    refresh_sessions(true)
    dlog("===== PLUGIN SETUP COMPLETE =====")
    dlog_tmux_state("setup final state")
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

function render_view(view_id)
    if tostring(view_id or "") == SETTINGS_VIEW_ID then
        render_settings_view()
        return
    end
    render()
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
        if event.action == ACTION_RENAME_TAB_EXISTING then
            prompt_rename_tab()
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

    if event.kind == "widget" and tostring(event.view_id or "") == SETTINGS_VIEW_ID then
        if handle_settings_event(event) then
            return
        end
    end

    if event.kind ~= "widget" or event.type ~= "button_click" then
        return
    end

    handle_button_action(event.id, event.x, event.y)
end
