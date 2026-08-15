# TermLab Plugin Support for VS Code

Lua language support for [TermLab](https://github.com/an0nn30/conch) terminal emulator plugins.

## Features

- **API Completions** — autocomplete for all TermLab plugin globals (`session`, `app`, `ui`, `crypto`, `net`) with parameter hints and documentation
- **Hover Docs** — hover over any API function to see its signature and description
- **Diagnostics** — runs `termlab check` on save to catch syntax errors, invalid API calls, and plugin header issues
- **Lifecycle Hints** — type information for panel plugin lifecycle functions (`setup`, `render`, `on_click`, `on_keybind`)

## Requirements

- [Lua Language Server](https://marketplace.visualstudio.com/items?itemName=sumneko.lua) (installed automatically as a dependency)
- `termlab` CLI on your PATH (for diagnostics)

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `termlab.checkOnSave` | `true` | Run `termlab check` on save |
| `termlab.executablePath` | `"termlab"` | Path to the `termlab` executable |

## Development

```bash
cd editors/vscode
npm install
npm run compile
```

To test locally, press F5 in VS Code to launch an Extension Development Host.

## Packaging

```bash
npm run package   # produces termlab-lua-0.1.0.vsix
```
