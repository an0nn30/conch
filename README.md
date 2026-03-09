<p align="center">
  <img src="crates/conch_app/icons/app-icon-512.png" alt="Conch" width="128" />
</p>

<h1 align="center">Conch</h1>

<p align="center">
  A fast, cross-platform terminal emulator and SSH manager with a Lua plugin system.<br/>
  Built in Rust. Runs on macOS, Windows, and Linux.
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

![Conch with Panels](docs/screenshot-panels.png)

## Why Conch?

Most terminal emulators do one thing well. SSH clients do another. File transfer tools are a third app entirely. Conch puts them all in one window — terminal, SSH sessions, SFTP file browser, and a plugin system that lets you build your own tools on top.

Think MobaXterm, but open source, cross-platform, and extensible.

## Features

**Terminal** — Full terminal emulation powered by [alacritty_terminal](https://github.com/alacritty/alacritty). 256-color, truecolor, mouse reporting, bracketed paste, tabs, multi-window.

**SSH Sessions** — Save connections with proxy jump support, organized in folders. Password and key authentication. Quick-connect from the sidebar.

**File Browser** — Side-by-side local and remote file browsing. Drag-and-drop SFTP uploads and downloads with progress tracking.

**SSH Tunnels** — Persistent local port forwarding you can activate and deactivate without closing your session.

**Lightweight** — ~80 MB memory, ~2% idle CPU. No Electron. Native GPU-accelerated rendering via OpenGL.

## Plugins

Conch has a **Lua 5.4 plugin system** that lets you extend the terminal with your own tools. Plugins run in a sandboxed environment and have access to a rich API.

### What can plugins do?

- Run commands on local or SSH sessions (silently, without touching your terminal)
- Show interactive form dialogs (text inputs, dropdowns, checkboxes)
- Display toast notifications with action buttons
- Build live-updating sidebar dashboards
- Encrypt and decrypt data, scan ports, resolve DNS
- Bind to custom keyboard shortcuts
- Set custom icons

### Three plugin types

| Type | Description | Example |
|------|-------------|---------|
| **Action** | Run-once scripts triggered from the menu or a keybinding | Encrypt/Decrypt tool |
| **Panel** | Persistent sidebar tabs with live-updating widgets | System monitor, Port scanner |
| **Bottom Panel** | Tabbed panels below the terminal for logs, monitoring, etc. | Service dashboard |

### Getting started with plugins

Drop a `.lua` file in `~/.config/conch/plugins/` and it appears in the Plugins tab. That's it.

```lua
-- name: Hello World
-- description: A simple action plugin
-- author: You

function setup()
    app.notify("Hello from a plugin!")
end
```

Conch ships with example plugins to get you started:

| Plugin | Type | What it does |
|--------|------|--------------|
| **System Info** | Panel | Live hostname, memory, disk, CPU load, top processes |
| **Port Scanner** | Panel | TCP port scanning with service identification |
| **Encrypt/Decrypt** | Action | AES encryption (CBC, GCM, ECB) with PBKDF2 key derivation |
| **Demo Bottom Panel** | Bottom Panel | Service dashboard with tables, stats, progress bars, live logs |

```bash
# Symlink the examples into your plugins directory
ln -s /path/to/rusty_conch/examples/plugins/*.lua ~/.config/conch/plugins/
```

### VS Code extension

The [Conch Plugin Support](editors/vscode/) extension provides Lua API completions, hover docs, and `conch check` diagnostics for plugin development.

See the full **[Plugin Documentation](docs/plugins.md)** for the complete API reference.

## Installation

### Download

Grab the latest build from the [Releases](https://github.com/an0nn30/rusty_conch/releases) page:

| Platform | Artifact |
|----------|----------|
| macOS (Universal) | `.dmg` |
| Windows | `.msi` installer or portable `.exe` |
| Linux (AMD64) | `.deb` / `.rpm` |
| Linux (ARM64) | `.deb` / `.rpm` |

### Build from source

Requires Rust 1.85+ (edition 2024).

```bash
git clone https://github.com/an0nn30/rusty_conch.git
cd rusty_conch
cargo build --release -p conch_app
```

<details>
<summary>Linux dependencies</summary>

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libgtk-3-dev libssl-dev pkg-config
```

</details>

## Keyboard Shortcuts

> On Linux/Windows, replace `Cmd` with `Ctrl`.

| Shortcut | Action |
|----------|--------|
| `Cmd+T` | New tab |
| `Cmd+W` | Close tab |
| `Cmd+1`–`9` | Switch to tab N |
| `Cmd+Shift+N` | New window |
| `Cmd+N` | New SSH connection |
| `Cmd+/` | Quick connect (toggle) |
| `Cmd+Shift+B` | Toggle file browser sidebar |
| `Cmd+Shift+E` | Toggle sessions sidebar |
| `Cmd+J` | Toggle bottom panel |
| `Cmd+Shift+F` | Focus file browser |
| `Cmd+Shift+P` | Plugin search |
| `Cmd+Shift+T` | SSH tunnels manager |
| `Cmd+Shift+Z` | Zen mode (hide all sidebars) |
| `Cmd+Q` | Quit |

All shortcuts are configurable — see [Configuration](#configuration) below.

## Configuration

Conch uses an Alacritty-compatible TOML config at `~/.config/conch/config.toml` (Linux/macOS) or `%APPDATA%\conch\config.toml` (Windows).

Standard [Alacritty config](https://alacritty.org/config-alacritty.html) sections (`[window]`, `[font]`, `[colors]`, `[terminal]`) work as-is. Conch adds its own sections:

```toml
[conch.keyboard]
new_tab = "cmd+t"
close_tab = "cmd+w"
new_window = "cmd+shift+n"
new_connection = "cmd+n"
toggle_left_sidebar = "cmd+shift+b"
toggle_right_sidebar = "cmd+shift+e"
toggle_bottom_panel = "cmd+j"

# Bind plugins to keyboard shortcuts
[conch.keyboard.plugins]
"system-info.open_panel" = "cmd+shift+i"
"encrypt-decrypt.run" = "cmd+shift+y"

[conch.ui]
native_menu_bar = false
font_size = 13.0
```

An example config is included in releases as `config.example.toml`.

## Project Structure

```
crates/
  conch_core/      Core data models, config, color schemes
  conch_session/   SSH/local session management, PTY, SFTP, tunnels
  conch_plugin/    Lua plugin runtime and API bindings
  conch_app/       GUI application (eframe/egui)
editors/
  vscode/          VS Code extension for plugin development
examples/
  plugins/         Example plugins (system-info, port-scanner, encrypt-decrypt, demo-bottom-panel)
```

## Contributing

Conch is early-stage and actively developed. Bug reports, feature requests, and pull requests are welcome.

## License

[Apache 2.0](LICENSE)
