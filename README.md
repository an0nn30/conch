# Conch

A cross-platform terminal emulator with SSH session management, built in Rust with [egui](https://github.com/emilk/egui).

[![Build macOS ARM64](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml)
[![Build macOS x86](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml)
[![Build Windows](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml)
[![Build Linux AMD64](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml)
[![Build Linux ARM64](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/an0nn30/rusty_conch/actions/workflows/release.yml)

![Conch Terminal](docs/screenshot-terminal.png)

![Conch with Panels](docs/screenshot-panels.png)

## Features

- **Terminal emulation** — Full terminal via [alacritty_terminal](https://github.com/alacritty/alacritty), supporting 256-color, truecolor, mouse reporting, bracketed paste, application cursor mode, and drag-and-drop file paths
- **SSH session management** — Saved connections with proxy jump/command support, organized in folders with inline edit/rename/delete; interactive password prompt when key auth is unavailable
- **Multi-window & tabs** — Multiple local and SSH sessions with Cmd+number switching, open extra windows with Cmd+Shift+N
- **File browser** — Dual-pane local/remote browser with SFTP upload/download and progress tracking
- **SSH tunnels** — Persistent local port forwarding with activate/deactivate
- **Lua plugin system** — Extend Conch with Lua 5.4 scripts: action plugins for one-shot tasks, panel plugins for live sidebar dashboards. Full API for session interaction, UI dialogs, cryptography, networking (port scanning, DNS), custom keybindings, and icons ([Plugin docs](docs/plugins.md))
- **Configurable** — Alacritty-compatible config format with Conch-specific extensions for keyboard shortcuts, cursor style, font, colors, and environment variables
- **Native feel** — Optional macOS native menu bar or transparent in-window title bar menu; window decorations configurable (full, transparent, buttonless, none)
- **IPC** — Control running instances via `conch msg new-window` / `conch msg new-tab` CLI commands
- **Cross-platform** — macOS (ARM64 + Intel), Windows, Linux (AMD64 + ARM64)

## Installation

### From Release

Download the latest release for your platform from the [Releases](https://github.com/an0nn30/rusty_conch/releases) page:

| Platform | Artifact |
|----------|----------|
| macOS (Apple Silicon) | `Conch-x.x.x-macos-arm64.dmg` |
| macOS (Intel) | `Conch-x.x.x-macos-x86_64.dmg` |
| Windows | `Conch-x.x.x-windows-x86_64.zip` |
| Linux (AMD64) | `.deb` / `.rpm` |
| Linux (ARM64) | `.deb` / `.rpm` |

### From Source

Requires Rust 1.85+ (edition 2024).

```bash
git clone https://github.com/an0nn30/rusty_conch.git
cd rusty_conch
cargo build --release -p conch_app
```

#### Linux Dependencies

```bash
sudo apt-get install -y \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libgtk-3-dev libssl-dev pkg-config
```

## Configuration

Conch uses an Alacritty-compatible TOML config with additional `[conch.*]` sections.

**Config location:** `~/.config/conch/config.toml` (Linux/macOS) or `%APPDATA%\conch\config.toml` (Windows)

See the [Alacritty config docs](https://alacritty.org/config-alacritty.html) for `[window]`, `[font]`, `[colors]`, and `[terminal]` sections. Conch adds:

```toml
[conch.keyboard]
new_tab = "cmd+t"
close_tab = "cmd+w"
new_window = "cmd+shift+n"
new_connection = "cmd+n"
quit = "cmd+q"
toggle_left_sidebar = "cmd+shift+b"
toggle_right_sidebar = "cmd+shift+e"
focus_quick_connect = "cmd+/"          # toggle: opens/closes right sidebar
focus_plugin_search = "cmd+shift+p"

[conch.keyboard.plugins]
"system-info.open_panel" = "cmd+shift+i"
"port-scanner.open_panel" = "cmd+shift+o"
"encrypt-decrypt.run" = "cmd+shift+y"

[conch.ui]
native_menu_bar = false    # true = macOS global menu, false = in-window menu
font_size = 13.0

[window]
decorations = "Full"       # Full, Transparent, Buttonless, or None
```

## Plugins

Conch has a Lua 5.4 plugin system for automating tasks, building tools, and extending the terminal.

- **Action plugins** — run-once scripts triggered from the Tools menu, sidebar, or keyboard shortcut
- **Panel plugins** — persistent sidebar tabs with live-updating dashboards (system info, port scanning, etc.)

Plugins can interact with local and SSH sessions (silently, without disturbing the terminal), show form dialogs, encrypt/decrypt data, scan ports, resolve DNS, declare custom keybindings, and set custom icons.

**Quick start:** Drop a `.lua` file in `~/.config/conch/plugins/` and it appears in the Plugins panel.

Conch ships with example plugins in `examples/plugins/` — symlink them to get started:

```bash
ln -s /path/to/rusty_conch/examples/plugins/*.lua ~/.config/conch/plugins/
```

| Plugin | Type | Description |
|--------|------|-------------|
| System Info | Panel | Live hostname, memory, disk, load, top processes (macOS/Linux) |
| Port Scanner | Panel | TCP port scanning with server dropdown, service identification |
| Encrypt/Decrypt | Action | AES encryption/decryption with PBKDF2 key derivation |

See the full **[Plugin System Documentation](docs/plugins.md)** for the complete API reference.

## Project Structure

```
crates/
  conch_core/      # Data models, config, color schemes (no framework deps)
  conch_session/   # SSH/local session management, PTY, SFTP, tunnels
  conch_plugin/    # Lua plugin runtime, API bindings (session, app, ui, crypto, net)
  conch_app/       # eframe/egui application, UI, terminal renderer
    src/
      app.rs           # Main application loop, dialogs, menus
      extra_window.rs  # Secondary window rendering & tab management
      mouse.rs         # Terminal mouse handling (selection + forwarding)
      input.rs         # Keyboard input → escape sequence translation
      state.rs         # Session, AppState, SessionBackend types
      terminal/        # Terminal widget, color conversion, size info
      ui/              # Sidebar, session panel, file browser, dialogs
      plugins.rs       # Plugin command handling, keybinding resolution
      shortcuts.rs     # Keyboard shortcut dispatch
      icons.rs         # Icon loading and texture cache
      ipc.rs           # Unix socket IPC listener
      macos_menu.rs    # Native macOS menu bar via objc2
packaging/
  macos/           # Info.plist for .app bundle
  linux/           # .desktop file
```

## License

MIT
