package termlab.plugin;

import java.util.ArrayList;
import java.util.List;

/**
 * Builder API for constructing TermLab widget trees.
 *
 * <p>Widgets are returned from {@link TermLabPlugin#render()} as a JSON array
 * string. This class provides a fluent builder that handles JSON serialization
 * so you never have to write raw JSON.</p>
 *
 * <h2>Usage</h2>
 * <pre>{@code
 * public String render() {
 *     return new Widgets()
 *         .heading("System Info")
 *         .separator()
 *         .keyValue("OS", System.getProperty("os.name"))
 *         .keyValue("Java", System.getProperty("java.version"))
 *         .button("refresh", "Refresh")
 *         .toJson();
 * }
 * }</pre>
 *
 * @see TermLabPlugin#render()
 */
public class Widgets {
    private final List<String> items = new ArrayList<>();

    // -- Layout Containers --------------------------------------------------

    /**
     * Add a horizontal row of child widgets.
     *
     * @param children widgets to lay out horizontally
     * @return this builder
     */
    public Widgets horizontal(Widgets children) {
        items.add("{\"type\":\"horizontal\",\"id\":null,\"children\":" + children.toJson() + ",\"spacing\":null}");
        return this;
    }

    /**
     * Add a vertical column of child widgets.
     *
     * @param children widgets to stack vertically
     * @return this builder
     */
    public Widgets vertical(Widgets children) {
        items.add("{\"type\":\"vertical\",\"id\":null,\"children\":" + children.toJson() + ",\"spacing\":null}");
        return this;
    }

    /**
     * Add a scrollable container.
     *
     * @param maxHeight maximum height in points, or {@code null} to fill
     * @param children  widgets inside the scroll area
     * @return this builder
     */
    public Widgets scrollArea(Float maxHeight, Widgets children) {
        items.add("{\"type\":\"scroll_area\",\"id\":null,\"max_height\":"
                + (maxHeight != null ? maxHeight : "null")
                + ",\"children\":" + children.toJson() + "}");
        return this;
    }

    // -- Data Display -------------------------------------------------------

    /**
     * Add a section heading (large, bold text).
     *
     * @param text heading text
     * @return this builder
     */
    public Widgets heading(String text) {
        items.add("{\"type\":\"heading\",\"text\":" + jsonStr(text) + "}");
        return this;
    }

    /**
     * Add a text label.
     *
     * @param text label text
     * @return this builder
     */
    public Widgets label(String text) {
        items.add("{\"type\":\"label\",\"text\":" + jsonStr(text) + ",\"style\":null}");
        return this;
    }

    /**
     * Add a styled text label.
     *
     * @param text  label text
     * @param style one of: {@code "secondary"}, {@code "muted"},
     *              {@code "accent"}, {@code "warn"}, {@code "error"}
     * @return this builder
     */
    public Widgets label(String text, String style) {
        items.add("{\"type\":\"label\",\"text\":" + jsonStr(text) + ",\"style\":" + jsonStr(style) + "}");
        return this;
    }

    /**
     * Add monospace text.
     *
     * @param text the text to display in a monospace font
     * @return this builder
     */
    public Widgets text(String text) {
        items.add("{\"type\":\"text\",\"text\":" + jsonStr(text) + "}");
        return this;
    }

    /**
     * Add a key-value pair (label on left, value on right).
     *
     * @param key   the label/key
     * @param value the value
     * @return this builder
     */
    public Widgets keyValue(String key, String value) {
        items.add("{\"type\":\"key_value\",\"key\":" + jsonStr(key) + ",\"value\":" + jsonStr(value) + "}");
        return this;
    }

    /**
     * Add a visual separator line.
     *
     * @return this builder
     */
    public Widgets separator() {
        items.add("{\"type\":\"separator\"}");
        return this;
    }

    /**
     * Add a flexible spacer.
     *
     * @return this builder
     */
    public Widgets spacer() {
        items.add("{\"type\":\"spacer\",\"size\":null}");
        return this;
    }

    /**
     * Add a fixed-size spacer.
     *
     * @param size size in points
     * @return this builder
     */
    public Widgets spacer(float size) {
        items.add("{\"type\":\"spacer\",\"size\":" + size + "}");
        return this;
    }

    /**
     * Add a status badge.
     *
     * @param text    badge text
     * @param variant one of: {@code "info"}, {@code "success"},
     *                {@code "warn"}, {@code "error"}
     * @return this builder
     */
    public Widgets badge(String text, String variant) {
        items.add("{\"type\":\"badge\",\"text\":" + jsonStr(text) + ",\"variant\":" + jsonStr(variant) + "}");
        return this;
    }

    /**
     * Add a progress bar.
     *
     * @param id       widget ID
     * @param fraction progress from 0.0 to 1.0
     * @param label    optional text label, or {@code null}
     * @return this builder
     */
    public Widgets progress(String id, float fraction, String label) {
        items.add("{\"type\":\"progress\",\"id\":" + jsonStr(id)
                + ",\"fraction\":" + fraction
                + ",\"label\":" + (label != null ? jsonStr(label) : "null") + "}");
        return this;
    }

    // -- Interactive Widgets ------------------------------------------------

    /**
     * Add a clickable button.
     *
     * <p>Generates a {@code button_click} event with the given {@code id}
     * when clicked.</p>
     *
     * @param id    widget ID (included in the click event)
     * @param label button label text
     * @return this builder
     */
    public Widgets button(String id, String label) {
        items.add("{\"type\":\"button\",\"id\":" + jsonStr(id) + ",\"label\":" + jsonStr(label) + ",\"icon\":null,\"enabled\":null}");
        return this;
    }

    /**
     * Add a button with an icon.
     *
     * @param id    widget ID
     * @param label button label
     * @param icon  icon name
     * @return this builder
     */
    public Widgets button(String id, String label, String icon) {
        items.add("{\"type\":\"button\",\"id\":" + jsonStr(id)
                + ",\"label\":" + jsonStr(label)
                + ",\"icon\":" + jsonStr(icon)
                + ",\"enabled\":null}");
        return this;
    }

    /**
     * Add a single-line text input.
     *
     * <p>Generates {@code text_input_changed} events as the user types and
     * {@code text_input_submit} on Enter (if enabled).</p>
     *
     * @param id    widget ID
     * @param value current text value
     * @param hint  placeholder text, or {@code null}
     * @return this builder
     */
    public Widgets textInput(String id, String value, String hint) {
        items.add("{\"type\":\"text_input\",\"id\":" + jsonStr(id)
                + ",\"value\":" + jsonStr(value)
                + ",\"hint\":" + (hint != null ? jsonStr(hint) : "null")
                + ",\"submit_on_enter\":true}");
        return this;
    }

    /**
     * Add a checkbox toggle.
     *
     * @param id      widget ID
     * @param label   checkbox label
     * @param checked current checked state
     * @return this builder
     */
    public Widgets checkbox(String id, String label, boolean checked) {
        items.add("{\"type\":\"checkbox\",\"id\":" + jsonStr(id)
                + ",\"label\":" + jsonStr(label)
                + ",\"checked\":" + checked + "}");
        return this;
    }

    // -- Raw JSON / Custom HTML -----------------------------------------------

    /**
     * Add a raw JSON widget string. Use this for widget types not yet
     * covered by the builder API.
     *
     * @param json a single widget JSON object
     * @return this builder
     */
    public Widgets raw(String json) {
        items.add(json);
        return this;
    }

    /**
     * Add raw HTML rendered in a Shadow DOM with CSS isolation.
     *
     * <p>Theme variables ({@code --bg}, {@code --fg}, {@code --green}, etc.)
     * are forwarded into the shadow root. Elements with a
     * {@code data-action="id"} attribute emit {@code button_click} events
     * back to the plugin when clicked.</p>
     *
     * @param content raw HTML string
     * @return this builder
     */
    public Widgets html(String content) {
        items.add("{\"type\":\"html\",\"content\":" + jsonStr(content) + "}");
        return this;
    }

    /**
     * Add raw HTML with custom CSS, rendered in a Shadow DOM.
     *
     * <p>The CSS is injected into the shadow root's {@code <style>} element
     * alongside the forwarded theme variables.</p>
     *
     * @param content raw HTML string
     * @param css     CSS rules for the shadow root
     * @return this builder
     */
    public Widgets html(String content, String css) {
        items.add("{\"type\":\"html\",\"content\":" + jsonStr(content)
                + ",\"css\":" + jsonStr(css) + "}");
        return this;
    }

    // -- Serialization ------------------------------------------------------

    /**
     * Serialize all added widgets to a JSON array string.
     *
     * <p>This is the value to return from {@link TermLabPlugin#render()}.</p>
     *
     * @return JSON array of widget objects
     */
    public String toJson() {
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < items.size(); i++) {
            if (i > 0) sb.append(",");
            sb.append(items.get(i));
        }
        sb.append("]");
        return sb.toString();
    }

    // -- Internal helpers ---------------------------------------------------

    /**
     * Escape and quote a string for JSON. Handles null, backslash,
     * double quotes, and control characters.
     */
    static String jsonStr(String s) {
        if (s == null) return "null";
        StringBuilder sb = new StringBuilder("\"");
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"':  sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\n': sb.append("\\n");  break;
                case '\r': sb.append("\\r");  break;
                case '\t': sb.append("\\t");  break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
            }
        }
        sb.append("\"");
        return sb.toString();
    }
}
