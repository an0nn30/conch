<p align="center">
  <img src="crates/conch_tauri/icons/icon.png" alt="Conch" width="128" />
</p>

<h1 align="center">Conch</h1>

<p align="center">
  A fast, cross-platform terminal emulator with built-in SSH, SFTP, and an extensible plugin system.<br/>
  Built with Rust + Tauri + xterm.js. Runs on macOS, Windows, and Linux.
</p>

<p align="center">
  <a href="https://github.com/an0nn30/rusty_conch/actions/workflows/ci.yml">
    <img src="https://github.com/an0nn30/rusty_conch/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://github.com/an0nn30/rusty_conch/releases">
    <img src="https://img.shields.io/github/v/release/an0nn30/rusty_conch?label=Download" alt="Latest Release" />
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="License" />
  </a>
</p>

---

## Why Conch?

Most terminal emulators do one thing well. SSH clients do another. File transfer tools are a third app entirely. Conch puts them all in one window — terminal, SSH sessions, SFTP file browser — and a plugin system that lets you build your own tools on top.

Think MobaXterm, but open source, cross-platform, and extensible.

## Features

**Terminal** — Full terminal emulation powered by [xterm.js](https://xtermjs.org/). 256-color, truecolor, mouse reporting, tabs, multi-window. Configurable font, cursor style, and scroll sensitivity.

**SSH Sessions** (built-in) — Save connections with proxy jump/command support, organized in folders. Password and key authentication. Quick-connect search from the sidebar. Parses `~/.ssh/config` automatically. Host key verification with `~/.ssh/known_hosts`.

**File Explorer** (built-in) — Dual-pane local and remote file browsing. Upload and download with real-time progress tracking via SFTP. Sortable columns, hidden file toggle, navigation history.

**SSH Tunnels** (built-in) — Local port forwarding with persistent tunnel definitions. Start/stop from the sidebar or the tunnel manager dialog.

**Credential Vault** — Encrypted credential storage using AES-256-GCM with Argon2id key derivation. Built-in SSH key generation (Ed25519, ECDSA, RSA), auto-lock timer, and vault-aware SSH connections. Accessible via Tools > Credential Vault.

**Settings Dialog** — Comprehensive settings UI accessible via File > Settings (or `Cmd+,`). Configure appearance, terminal, shell, keyboard shortcuts, plugins, and advanced settings with an Apply/Cancel workflow.

**Theming** — Full [Alacritty-compatible](https://github.com/alacritty/alacritty-theme) `.toml` theme support. Drop a theme file in `~/.config/conch/themes/` and set `[colors] theme = "name"`. Hot-reload on file change. Live preview in the Settings dialog shows theme colors before applying.

**Zen Mode** — `Cmd+Shift+Z` hides all panels for a distraction-free terminal.

**Lightweight** — No Electron. Tauri webview with a Rust backend. Near-zero idle CPU usage.

## Plugin System

Conch supports **Lua** and **Java** plugins for extending functionality with custom panels, menu items, notifications, dialogs, and inter-plugin communication.

### What can plugins do?

- **Register sidebar panels** with live-updating declarative widgets (trees, tables, buttons, text inputs, etc.)
- **Communicate with other plugins** via pub/sub events and RPC queries
- **Show dialogs** — forms, confirmations, prompts, alerts
- **Access the clipboard**, show notifications, register menu items with keyboard shortcuts
- **Write to the terminal**, open new tabs
- **Persist configuration** via a per-plugin key-value config store

### Java plugins

Conch supports **Java plugins** via an embedded JVM. Any JVM language works (Java, Kotlin, Scala, Groovy). The SDK JAR is embedded in the binary — no external files needed. Java plugins have full access to logging, menu items, notifications, clipboard, dialogs (prompt, confirm, alert, forms), config persistence, inter-plugin communication, and terminal/tab control.

See the [Java Plugin SDK](java-sdk/) for the API reference.

### Lua plugins

Lightweight **Lua 5.4 plugins** for quick scripting. Drop a `.lua` file in your plugins directory and enable it via Settings > Plugins.

```lua
-- plugin-name: Hello World
-- plugin-description: A simple action plugin
-- plugin-type: action
-- plugin-version: 1.0.0

function setup()
    app.log("info", "Hello from a plugin!")
    app.register_menu_item("Tools", "Say Hello", "say_hello")
end

function on_event(event)
    if type(event) == "table" and event.action == "say_hello" then
        app.notify("Hello", "Hello from a plugin!", "success")
    end
end
```

### Plugin management

Plugins are managed via **Settings > Plugins**:
- Scans all configured search paths for `.lua` and `.jar` plugins
- Enable/disable plugins with a single click
- Enabled plugins are remembered across restarts
- Plugin menu items appear in the native Tools menu

### Plugin development

See the **[VS Code extension](editors/vscode/)** or **[Neovim definitions](editors/neovim/)** for Lua API completions and hover docs.

## Installation

### Pre-built binaries

Download the latest release for your platform from the [Releases page](https://github.com/an0nn30/rusty_conch/releases).

### Build from source

Requires Rust 1.85+ (edition 2024) and a JDK (for Java plugin support).

```bash
git clone https://github.com/an0nn30/rusty_conch.git
cd rusty_conch
cargo build --release -p conch_tauri
```

The binary is at `target/release/conch`.

<details>
<summary>Linux dependencies</summary>

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libssl-dev pkg-config \
  libayatana-appindicator3-dev librsvg2-dev
```

</details>

## Keyboard Shortcuts

> On Linux/Windows, replace `Cmd` with `Ctrl`.

| Shortcut | Action |
|----------|--------|
| `Cmd+,` | Settings |
| `Cmd+T` | New tab |
| `Cmd+W` | Close tab |
| `Cmd+1`--`9` | Switch to tab N |
| `Cmd+Shift+N` | New window |
| `Cmd+Shift+E` | Toggle file explorer (left panel) |
| `Cmd+Shift+R` | Toggle sessions panel (right panel) |
| `Cmd+Shift+J` | Toggle bottom panel |
| `Cmd+Shift+Z` | Zen mode (hide all panels) |
| `Cmd+/` | Toggle & focus quick connect |
| `Cmd+=` / `Cmd+-` / `Cmd+0` | Zoom in / out / reset |
| `Cmd+Shift+T` | Manage SSH tunnels |

All shortcuts are configurable in `[conch.keyboard]`. Plugins can also register their own keybindings.

## Configuration

Most settings can be configured through the Settings dialog (File > Settings or `Cmd+,`). For manual editing, Conch uses a TOML config at `~/.config/conch/config.toml` (Linux/macOS) or `%APPDATA%\conch\config.toml` (Windows).

Alacritty-compatible sections (`[window]`, `[font]`, `[colors]`, `[terminal]`) work as-is. Conch adds its own sections:

```toml
[colors]
theme = "dracula"           # Any Alacritty .toml theme file name

[conch.keyboard]
new_tab = "cmd+t"
close_tab = "cmd+w"
zen_mode = "cmd+shift+z"
toggle_left_panel = "cmd+shift+e"
toggle_right_panel = "cmd+shift+r"

[conch.plugins]
enabled = true              # Master switch
lua = true                  # Lua plugins
java = true                 # Java plugins (disabling skips JVM startup)
search_paths = []           # Additional plugin discovery directories
```

See [`config.example.toml`](config.example.toml) for the full reference.

## Project Structure

```
crates/
  conch_core/         Config loading, color schemes, persistent state
  conch_plugin_sdk/   Widget/event types shared with Lua and Java plugins
  conch_plugin/       Plugin host — message bus, Lua runner, Java runtime
  conch_remote/       Platform-agnostic SSH, SFTP, tunnels (russh)
  conch_tauri/        The app — Tauri/xterm.js UI, built-in SSH/SFTP/tunnels
  conch_vault/        Credential vault — encrypted storage, key generation
java-sdk/             Java Plugin SDK (JAR + sources)
editors/
  vscode/             VS Code extension for Lua plugin development
  neovim/             Neovim/LuaLS type definitions for Lua plugin development
```

## Contributing

Conch is actively developed. Bug reports, feature requests, and pull requests are welcome — please [open an issue](https://github.com/an0nn30/rusty_conch/issues) to start a discussion. When submitting PRs, use branch prefixes: `feat/`, `fix/`, `chore/`, or `perf/`.

For plugin development, see the [Java Plugin SDK](java-sdk/) and the editor extensions in [editors/](editors/).

## License

[Apache 2.0](LICENSE)
