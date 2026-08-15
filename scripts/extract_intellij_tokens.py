#!/usr/bin/env python3
"""Generate design-system token CSS + terminal theme from the JVM TermLab repo.

Reads:
  <termlab-repo>/core/resources/themes/TermLabDark.theme.json
  <termlab-repo>/core/resources/themes/TermLabLight.theme.json
  <termlab-repo>/core/resources/termlab-dark.xml

Writes (under --out-dir, default crates/termlab_tauri/frontend):
  styles/design-system/tokens-dark.css   (:root)
  styles/design-system/tokens-light.css  (:root[data-tl-appearance="light"])
  themes/TermLab Dark.toml               (Alacritty-format terminal theme)

Generated files are committed. Never hand-edit them; semantic aliases live in
styles/design-system/base.css instead.
"""
import argparse
import json
import re
import xml.etree.ElementTree as ET
from pathlib import Path

BANNER = "/* GENERATED FILE — do not edit. Run scripts/extract_intellij_tokens.py */"


def _is_valid_css_color(value):
    """Check if a value is a valid CSS color."""
    if not isinstance(value, str):
        return False
    # Accept hex colors (#rgb, #rrggbb, #rrggbbaa)
    if re.match(r'^#[0-9a-fA-F]{3}([0-9a-fA-F]{3})?([0-9a-fA-F]{2})?$', value):
        return True
    # Accept rgba() and rgb() functions
    if re.match(r'^rgba?\s*\(', value):
        return True
    # Accept standard CSS named colors (whitelist of common ones)
    named_colors = {
        'aliceblue', 'antiquewhite', 'aqua', 'aquamarine', 'azure', 'beige', 'bisque', 'black',
        'blanchedalmond', 'blue', 'blueviolet', 'brown', 'burlywood', 'cadetblue', 'chartreuse',
        'chocolate', 'coral', 'cornflowerblue', 'cornsilk', 'crimson', 'cyan', 'darkblue',
        'darkcyan', 'darkgoldenrod', 'darkgray', 'darkgrey', 'darkgreen', 'darkkhaki',
        'darkmagenta', 'darkolivegreen', 'darkorange', 'darkorchid', 'darkred', 'darksalmon',
        'darkseagreen', 'darkslateblue', 'darkslategray', 'darkslategrey', 'darkturquoise',
        'darkviolet', 'deeppink', 'deepskyblue', 'dimgray', 'dimgrey', 'dodgerblue',
        'firebrick', 'floralwhite', 'forestgreen', 'fuchsia', 'gainsboro', 'ghostwhite',
        'gold', 'goldenrod', 'gray', 'grey', 'green', 'greenyellow', 'honeydew', 'hotpink',
        'indianred', 'indigo', 'ivory', 'khaki', 'lavender', 'lavenderblush', 'lawngreen',
        'lemonchiffon', 'lightblue', 'lightcoral', 'lightcyan', 'lightgoldenrodyellow',
        'lightgray', 'lightgrey', 'lightgreen', 'lightpink', 'lightsalmon', 'lightseagreen',
        'lightskyblue', 'lightslategray', 'lightslategrey', 'lightsteelblue', 'lightyellow',
        'lime', 'limegreen', 'linen', 'magenta', 'maroon', 'mediumaquamarine', 'mediumblue',
        'mediumorchid', 'mediumpurple', 'mediumseagreen', 'mediumslateblue', 'mediumspringgreen',
        'mediumturquoise', 'mediumvioletred', 'midnightblue', 'mintcream', 'mistyrose',
        'moccasin', 'navajowhite', 'navy', 'oldlace', 'olive', 'olivedrab', 'orange',
        'orangered', 'orchid', 'palegoldenrod', 'palegreen', 'paleturquoise', 'palevioletred',
        'papayawhip', 'peachpuff', 'peru', 'pink', 'plum', 'powderblue', 'purple', 'red',
        'rosybrown', 'royalblue', 'saddlebrown', 'salmon', 'sandybrown', 'seagreen', 'seashell',
        'sienna', 'silver', 'skyblue', 'slateblue', 'slategray', 'slategrey', 'snow',
        'springgreen', 'steelblue', 'tan', 'teal', 'thistle', 'tomato', 'turquoise', 'violet',
        'wheat', 'white', 'whitesmoke', 'yellow', 'yellowgreen',
    }
    if value.lower() in named_colors:
        return True
    return False


def _resolve(value, palette):
    """Resolve a theme.json value to a color string, or None to skip."""
    if isinstance(value, dict):
        value = value.get("os.mac", value.get("os.default"))
    if not isinstance(value, str):
        return None
    return palette.get(value, value)


def _flatten(ui, palette, prefix, out, on_warning=None):
    for key, value in ui.items():
        name = "base" if key == "*" else key
        token = f"{prefix}-{name}" if prefix else name
        if isinstance(value, dict) and not ("os.default" in value or "os.mac" in value):
            _flatten(value, palette, token, out, on_warning)
        else:
            resolved = _resolve(value, palette)
            if resolved is not None:
                token_key = token.replace(".", "-")
                out[token_key] = resolved
                # Validate that the resolved value is a valid CSS color
                if not _is_valid_css_color(resolved):
                    if on_warning:
                        on_warning(f"--tl-{token_key}: {resolved}")


def theme_to_css(theme, selector, on_warning=None):
    palette = theme.get("colors", {})
    tokens = {}
    _flatten(theme.get("ui", {}), palette, "", tokens, on_warning)
    lines = [f"{selector} {{", "  " + BANNER]
    for name, value in sorted(tokens.items()):
        lines.append(f"  --tl-{name}: {value};")
    lines.append("}")
    return "\n".join(lines) + "\n"


# ANSI slot -> (alacritty table, alacritty key)
_ANSI = {
    "CONSOLE_BLACK_OUTPUT": ("normal", "black"),
    "CONSOLE_RED_OUTPUT": ("normal", "red"),
    "CONSOLE_GREEN_OUTPUT": ("normal", "green"),
    "CONSOLE_YELLOW_OUTPUT": ("normal", "yellow"),
    "CONSOLE_BLUE_OUTPUT": ("normal", "blue"),
    "CONSOLE_MAGENTA_OUTPUT": ("normal", "magenta"),
    "CONSOLE_CYAN_OUTPUT": ("normal", "cyan"),
    "CONSOLE_GRAY_OUTPUT": ("normal", "white"),
    "CONSOLE_DARKGRAY_OUTPUT": ("bright", "black"),
    "CONSOLE_RED_BRIGHT_OUTPUT": ("bright", "red"),
    "CONSOLE_GREEN_BRIGHT_OUTPUT": ("bright", "green"),
    "CONSOLE_YELLOW_BRIGHT_OUTPUT": ("bright", "yellow"),
    "CONSOLE_BLUE_BRIGHT_OUTPUT": ("bright", "blue"),
    "CONSOLE_MAGENTA_BRIGHT_OUTPUT": ("bright", "magenta"),
    "CONSOLE_CYAN_BRIGHT_OUTPUT": ("bright", "cyan"),
    "CONSOLE_WHITE_OUTPUT": ("bright", "white"),
}


def _hex(value):
    value = value.strip().lstrip("#")
    return "#" + value if re.fullmatch(r"[0-9a-fA-F]{6}", value) else None


def scheme_to_alacritty(xml_text):
    root = ET.fromstring(xml_text)
    normal, bright = {}, {}
    background = foreground = None
    for opt in root.iter("option"):
        name = opt.get("name", "")
        if name == "CONSOLE_BACKGROUND_KEY" and opt.get("value"):
            background = _hex(opt.get("value"))
        if name in _ANSI:
            fg = opt.find("./value/option[@name='FOREGROUND']")
            if fg is not None and _hex(fg.get("value", "")):
                table, key = _ANSI[name]
                (normal if table == "normal" else bright)[key] = _hex(fg.get("value"))
    foreground = normal.get("white") or "#abb2bf"
    lines = ["# GENERATED — scripts/extract_intellij_tokens.py", "[colors.primary]"]
    if background:
        lines.append(f'background = "{background}"')
    lines.append(f'foreground = "{foreground}"')
    for table_name, table in (("normal", normal), ("bright", bright)):
        lines.append(f"[colors.{table_name}]")
        for key in ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"):
            if key in table:
                lines.append(f'{key} = "{table[key]}"')
    return "\n".join(lines) + "\n"


def main():
    import sys

    ap = argparse.ArgumentParser()
    ap.add_argument("--termlab-repo", default=str(Path(__file__).resolve().parents[2] / "TermLab"))
    ap.add_argument("--out-dir", default="crates/termlab_tauri/frontend")
    args = ap.parse_args()

    repo = Path(args.termlab_repo)
    out = Path(args.out_dir)
    themes_dir = repo / "core/resources/themes"

    def warn(msg):
        print(f"warning: invalid CSS color in token {msg}", file=sys.stderr)

    ds = out / "styles/design-system"
    ds.mkdir(parents=True, exist_ok=True)
    dark = json.loads((themes_dir / "TermLabDark.theme.json").read_text())
    light = json.loads((themes_dir / "TermLabLight.theme.json").read_text())
    (ds / "tokens-dark.css").write_text(theme_to_css(dark, ":root", on_warning=warn))
    (ds / "tokens-light.css").write_text(
        theme_to_css(light, ':root[data-tl-appearance="light"]', on_warning=warn))

    theme_out = out / "themes"
    theme_out.mkdir(parents=True, exist_ok=True)
    xml_text = (repo / "core/resources/termlab-dark.xml").read_text()
    (theme_out / "TermLab Dark.toml").write_text(scheme_to_alacritty(xml_text))
    print("wrote tokens-dark.css, tokens-light.css, TermLab Dark.toml")


if __name__ == "__main__":
    main()
