"""Golden tests for the IntelliJ theme token extractor. Run directly:
python3 scripts/tests/test_extract_tokens.py"""
import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from extract_intellij_tokens import theme_to_css, scheme_to_alacritty

THEME = {
    "name": "TermLab Dark",
    "dark": True,
    "colors": {"accentColor": "#6B80A1", "backgroundColor": "#21252b"},
    "ui": {
        "*": {
            "background": "backgroundColor",
            "foreground": "#abb2bf",
            "selectionBackground": {"os.default": "#111111", "os.mac": "#323844"},
        },
        "ActionButton": {"hoverBackground": "#3d424b"},
        "ToolWindow": {"Header": {"background": "accentColor"}},
    },
}

LIGHT_THEME = {
    "name": "TermLab Light",
    "dark": False,
    "colors": {
        "background": "#E3E8EF",
        "foreground": "#1F2933",
        "selectionBackground": "#CAD4E2",
    },
    "ui": {
        "*": {
            "background": "background",
            "foreground": "foreground",
            "infoForeground": "#8A94A3",
            "borderColor": "#C5CDD6",
            "selectionBackground": "selectionBackground",
        },
        "Panel": {"background": "#E3E8EF"},
    },
}

SCHEME_XML = """<?xml version="1.0"?>
<scheme name="TermLab Dark" version="142">
  <colors>
    <option name="CONSOLE_BACKGROUND_KEY" value="070A0E" />
  </colors>
  <attributes>
    <option name="CONSOLE_BLACK_OUTPUT">
      <value><option name="FOREGROUND" value="3c4048" /></value>
    </option>
    <option name="CONSOLE_RED_OUTPUT">
      <value><option name="FOREGROUND" value="e06c75" /></value>
    </option>
  </attributes>
</scheme>"""


def test_theme_to_css():
    css = theme_to_css(THEME, selector=":root")
    assert "--tl-base-background: #21252b;" in css, css          # named ref resolved
    assert "--tl-base-foreground: #abb2bf;" in css               # literal passthrough
    assert "--tl-base-selectionBackground: #323844;" in css      # os.mac wins
    assert "--tl-ActionButton-hoverBackground: #3d424b;" in css  # component key
    assert "--tl-ToolWindow-Header-background: #6B80A1;" in css  # nested + named ref
    assert css.strip().startswith(":root {")
    assert "GENERATED FILE" in css                               # do-not-edit banner


def test_theme_to_css_light_source():
    """Light-source golden: minimal light theme JSON -> exact light-appearance
    output, including the :root[data-tl-appearance="light"] wrapper selector
    that scripts/extract_intellij_tokens.py's main() uses for tokens-light.css."""
    css = theme_to_css(LIGHT_THEME, selector=':root[data-tl-appearance="light"]')
    assert css.strip().startswith(':root[data-tl-appearance="light"] {')
    assert "GENERATED FILE" in css
    assert "--tl-base-background: #E3E8EF;" in css       # named ref resolved from light palette
    assert "--tl-base-foreground: #1F2933;" in css
    assert "--tl-base-infoForeground: #8A94A3;" in css    # literal passthrough
    assert "--tl-base-borderColor: #C5CDD6;" in css
    assert "--tl-base-selectionBackground: #CAD4E2;" in css
    assert "--tl-Panel-background: #E3E8EF;" in css       # component key


def test_scheme_to_alacritty():
    toml_text = scheme_to_alacritty(SCHEME_XML)
    assert 'black = "#3c4048"' in toml_text
    assert 'red = "#e06c75"' in toml_text
    assert 'background = "#070A0E"' in toml_text


def test_unresolved_reference_warning():
    """Test that unresolved named references produce warnings."""
    theme_with_invalid = {
        "name": "Test",
        "dark": True,
        "colors": {"accentColor": "#6B80A1"},
        "ui": {
            "*": {
                "background": "accentColor",
                "foreground": "invalidColorName",  # typo or unresolved reference
            },
        },
    }
    warnings = []
    css = theme_to_css(theme_with_invalid, selector=":root", on_warning=lambda msg: warnings.append(msg))

    # Verify the token is still emitted (warn, don't drop)
    assert "--tl-base-foreground: invalidColorName;" in css
    # Verify a warning was generated for the invalid color
    assert len(warnings) > 0
    assert "invalidColorName" in warnings[0]
    assert "--tl-base-foreground" in warnings[0]


if __name__ == "__main__":
    test_theme_to_css()
    test_theme_to_css_light_source()
    test_scheme_to_alacritty()
    test_unresolved_reference_warning()
    print("ok")
