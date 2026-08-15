# TermLab Lua Plugin SDK — Neovim / LazyVim Support

Type definitions for [LuaLS (lua-language-server)](https://github.com/LuaLS/lua-language-server) that provide autocompletion, hover documentation, and type checking when writing TermLab Lua plugins in Neovim.

## Setup

### Option 1: Copy `.luarc.json` into your plugin directory

Copy the `.luarc.json` from this directory into the directory where you write your TermLab plugins, and update the library path to point to the stubs:

```json
{
  "runtime": { "version": "Lua 5.4" },
  "workspace": {
    "library": ["/path/to/termlab/editors/neovim/lua/termlab"],
    "checkThirdParty": false
  },
  "diagnostics": {
    "globals": ["ui", "app", "session", "net", "setup", "render", "on_event", "on_query"]
  }
}
```

### Option 2: Add to LuaLS global library (all Lua files)

In your Neovim config (`lua/plugins/lsp.lua` or equivalent):

```lua
require("lspconfig").lua_ls.setup({
  settings = {
    Lua = {
      workspace = {
        library = {
          -- Add TermLab stubs to every Lua workspace
          "/path/to/termlab/editors/neovim/lua/termlab",
        },
      },
      diagnostics = {
        globals = { "ui", "app", "session", "net", "setup", "render", "on_event", "on_query" },
      },
    },
  },
})
```

### Option 3: LazyVim — via `neoconf.nvim`

If you use LazyVim with `neoconf.nvim`, create a `.neoconf.json` in your plugin workspace:

```json
{
  "lspconfig": {
    "lua_ls": {
      "Lua.workspace.library": ["/path/to/termlab/editors/neovim/lua/termlab"],
      "Lua.diagnostics.globals": ["ui", "app", "session", "net", "setup", "render", "on_event", "on_query"]
    }
  }
}
```

## What you get

- **Autocompletion** — `ui.`, `app.`, `session.`, `net.` show all available functions
- **Hover docs** — parameter names, types, descriptions, and code examples
- **Type checking** — event types, return types, and optional parameters
- **Signature help** — parameter hints as you type function calls

## Stubs included

| File | API |
|------|-----|
| `ui.lua` | Widget rendering, layout containers, dialogs |
| `app.lua` | Logging, clipboard, events, notifications, config, menus, IPC |
| `session.lua` | Terminal session, command execution, platform info |
| `net.lua` | DNS resolution, port scanning, time |
| `lifecycle.lua` | Plugin callbacks (`setup`, `render`, `on_event`, `on_query`) and event types |
