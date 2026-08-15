--- TermLab Plugin SDK — net.* API
--- LuaLS type definitions for autocompletion and hover docs.
--- https://github.com/an0nn30/conch
---
--- These are stubs only — do NOT require() this file in your plugin.

---@meta

---@class net
---The `net` table provides network utilities: DNS resolution, port scanning,
---and time.
net = {}

---Get the current Unix timestamp as a floating-point number.
---@return number seconds Seconds since Unix epoch (with sub-second precision)
function net.time() end

---Resolve a hostname to a list of IP addresses.
---@param host string Hostname to resolve
---@return string[] addresses List of resolved IP address strings
function net.resolve(host) end

---Scan TCP ports on a host.
---
---```lua
---local open = net.scan("192.168.1.1", {22, 80, 443}, 500)
---for _, result in ipairs(open) do
---  print(result.port, result.open)
---end
---```
---@param host string Target hostname or IP
---@param ports integer[] List of ports to scan
---@param timeout_ms? integer Connection timeout per port in ms (default 1000)
---@param concurrency? integer Max concurrent connections (reserved, not yet used)
---@return TermLabScanResult[] results Only open ports are returned
function net.scan(host, ports, timeout_ms, concurrency) end

-- ═══════════════════════════════════════════════════════════════════════
-- Types
-- ═══════════════════════════════════════════════════════════════════════

---@class TermLabScanResult
---@field port integer Port number
---@field open boolean Always true (only open ports are returned)

return net
