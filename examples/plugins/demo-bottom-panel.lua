-- plugin-name: Demo Bottom Panel
-- plugin-description: Showcase of bottom-panel widgets — tables, logs, progress, key-values
-- plugin-type: bottom-panel
-- plugin-version: 1.0.0
-- plugin-keybind: open_panel = cmd+shift+d | Toggle Demo Bottom Panel

-- Simulated log buffer
local log_lines = {}
local tick = 0

local services = {
    { name = "nginx",    status = "running", pid = "1234", cpu = "0.3%",  mem = "48 MB"  },
    { name = "postgres", status = "running", pid = "2345", cpu = "1.2%",  mem = "256 MB" },
    { name = "redis",    status = "running", pid = "3456", cpu = "0.1%",  mem = "12 MB"  },
    { name = "node-api", status = "running", pid = "4567", cpu = "2.8%",  mem = "180 MB" },
    { name = "celery",   status = "stopped", pid = "--",   cpu = "--",    mem = "--"      },
    { name = "cron",     status = "running", pid = "5678", cpu = "0.0%",  mem = "4 MB"   },
}

local log_templates = {
    "[INFO]  nginx: GET /api/health 200 OK (2ms)",
    "[INFO]  nginx: POST /api/data 201 Created (45ms)",
    "[WARN]  postgres: slow query detected (320ms) — SELECT * FROM events",
    "[INFO]  redis: PING — PONG (0.1ms)",
    "[INFO]  node-api: request processed — user_id=42",
    "[ERROR] celery: task failed — TimeoutError on send_email",
    "[INFO]  nginx: GET /static/app.js 304 Not Modified",
    "[INFO]  postgres: connection established — pool size 8/20",
    "[WARN]  node-api: memory usage approaching threshold (175 MB / 200 MB)",
    "[INFO]  cron: scheduled job 'cleanup_logs' completed",
    "[INFO]  nginx: GET /api/users?page=3 200 OK (18ms)",
    "[ERROR] node-api: unhandled rejection — Cannot read property 'id' of undefined",
    "[INFO]  redis: SET session:abc123 OK (0.2ms)",
    "[INFO]  postgres: vacuum completed on table 'events' — 1,204 dead tuples removed",
    "[WARN]  nginx: upstream response time exceeded 500ms for /api/reports",
}

function setup()
    -- Seed with some initial log lines
    for i = 1, 8 do
        local ts = string.format("2025-03-09 10:%02d:%02d", math.random(0, 59), math.random(0, 59))
        table.insert(log_lines, ts .. "  " .. log_templates[math.random(#log_templates)])
    end
    ui.set_refresh(3)
end

function render()
    tick = tick + 1
    ui.panel_clear()

    -- Section 1: Service status table
    ui.panel_heading("Services")

    local columns = { "Service", "Status", "PID", "CPU", "Memory" }
    local rows = {}
    for _, svc in ipairs(services) do
        table.insert(rows, { svc.name, svc.status, svc.pid, svc.cpu, svc.mem })
    end
    ui.panel_table(columns, rows)

    ui.panel_separator()

    -- Section 2: Key-value stats
    ui.panel_heading("Cluster Stats")
    local running = 0
    for _, svc in ipairs(services) do
        if svc.status == "running" then running = running + 1 end
    end
    ui.panel_kv("Services:", running .. " / " .. #services .. " running")
    ui.panel_kv("Uptime:", string.format("%dd %dh %dm", math.random(1, 30), math.random(0, 23), math.random(0, 59)))
    ui.panel_kv("Requests/sec:", tostring(math.random(120, 450)))
    ui.panel_kv("Avg latency:", math.random(5, 85) .. " ms")
    ui.panel_kv("Error rate:", string.format("%.2f%%", math.random(0, 300) / 100))

    ui.panel_separator()

    -- Section 3: Progress bars
    ui.panel_heading("Resources")
    local cpu_pct = math.random(15, 75) / 100
    local mem_pct = math.random(40, 85) / 100
    local disk_pct = math.random(20, 60) / 100
    ui.panel_progress("CPU", cpu_pct, string.format("%.0f%%", cpu_pct * 100))
    ui.panel_progress("Memory", mem_pct, string.format("%.1f / 8.0 GB", mem_pct * 8))
    ui.panel_progress("Disk", disk_pct, string.format("%.0f / 500 GB", disk_pct * 500))

    ui.panel_separator()

    -- Section 4: Scrollable log output
    ui.panel_heading("Live Logs")

    -- Append a few new log lines each tick
    local num_new = math.random(1, 3)
    for _ = 1, num_new do
        local h = 10 + math.floor(tick / 20)
        local m = math.random(0, 59)
        local s = math.random(0, 59)
        local ts = string.format("2025-03-09 %02d:%02d:%02d", h % 24, m, s)
        table.insert(log_lines, ts .. "  " .. log_templates[math.random(#log_templates)])
    end

    -- Keep the last 200 lines
    while #log_lines > 200 do
        table.remove(log_lines, 1)
    end

    ui.panel_scroll_text(log_lines)

    ui.panel_separator()

    -- Section 5: Action buttons
    ui.panel_button("restart_all", "Restart All Services")
    ui.panel_button("clear_logs", "Clear Logs")
end

function on_click(button_id)
    if button_id == "clear_logs" then
        log_lines = {}
        app.notify("Logs cleared", "info")
    elseif button_id == "restart_all" then
        -- Simulate restarting stopped services
        for _, svc in ipairs(services) do
            if svc.status == "stopped" then
                svc.status = "running"
                svc.pid = tostring(math.random(6000, 9999))
                svc.cpu = "0.0%"
                svc.mem = "8 MB"
            end
        end
        app.notify("All services restarted", "success")
    end
end
