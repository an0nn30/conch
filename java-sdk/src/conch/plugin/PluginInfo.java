package conch.plugin;

/**
 * Metadata describing a Conch plugin.
 *
 * <p>Returned by {@link ConchPlugin#getInfo()} during discovery and loading.
 * The fields are displayed in the Plugin Manager UI and used by the host to
 * determine how the plugin is presented.</p>
 *
 * <h2>Plugin Types</h2>
 * <ul>
 *   <li>{@code "tool_window"} — the plugin renders a dockable tool window
 *       using widgets or raw HTML. The host calls {@link ConchPlugin#render()}
 *       to obtain the content. The user can move the tool window between
 *       zones (left-top, left-bottom, right-top, right-bottom) at runtime.</li>
 *   <li>{@code "action"} — the plugin runs in the background and interacts
 *       only through menu items, events, and host API calls. No tool window
 *       is created.</li>
 * </ul>
 *
 * <h2>Default Zone (Tool-Window plugins only)</h2>
 * <ul>
 *   <li>{@code "left"} — left sidebar (default zone: left-top)</li>
 *   <li>{@code "right"} — right sidebar (default zone: right-top)</li>
 *   <li>{@code "bottom"} — bottom panel</li>
 *   <li>{@code "none"} — no default zone (for action plugins)</li>
 * </ul>
 *
 * <p>The default zone is only used on first launch; after that, the user's
 * persisted layout takes precedence.</p>
 *
 * <h2>Example</h2>
 * <pre>{@code
 * // Action plugin (no tool window):
 * new PluginInfo("My Tool", "Does useful things", "1.0.0");
 *
 * // Tool-window plugin (right sidebar):
 * new PluginInfo("File Browser", "Browse remote files", "0.2.0", "tool_window", "right");
 * }</pre>
 *
 * @see ConchPlugin#getInfo()
 */
public class PluginInfo {

    /**
     * Display name of the plugin.
     *
     * <p>Shown in the Plugin Manager and used as the plugin's unique
     * identifier. Must be non-null and non-empty.</p>
     */
    public final String name;

    /**
     * Short description of what the plugin does.
     *
     * <p>Shown in the Plugin Manager UI beneath the plugin name.</p>
     */
    public final String description;

    /**
     * Semantic version string (e.g. {@code "1.0.0"}, {@code "0.3.2-beta"}).
     */
    public final String version;

    /**
     * Plugin type: {@code "tool_window"} or {@code "action"}.
     *
     * <p>Tool-window plugins render widgets via {@link ConchPlugin#render()}.
     * Action plugins run in the background without a UI panel.</p>
     *
     * <p>The legacy value {@code "panel"} is accepted as an alias for
     * {@code "tool_window"}.</p>
     */
    public final String pluginType;

    /**
     * Default zone for the tool window: {@code "left"}, {@code "right"},
     * {@code "bottom"}, or {@code "none"}.
     *
     * <p>Ignored for action plugins. The user can move the tool window
     * to any zone at runtime.</p>
     */
    public final String panelLocation;

    /**
     * Create plugin metadata with full control over type and default zone.
     *
     * @param name          display name (unique identifier)
     * @param description   short description
     * @param version       semantic version string
     * @param pluginType    {@code "tool_window"} or {@code "action"}
     * @param panelLocation {@code "left"}, {@code "right"}, {@code "bottom"},
     *                      or {@code "none"}
     */
    public PluginInfo(String name, String description, String version,
                      String pluginType, String panelLocation) {
        this.name = name;
        this.description = description;
        this.version = version;
        this.pluginType = pluginType;
        this.panelLocation = panelLocation;
    }

    /**
     * Create metadata for an action plugin (no tool window).
     *
     * <p>Equivalent to
     * {@code new PluginInfo(name, description, version, "action", "none")}.</p>
     *
     * @param name        display name
     * @param description short description
     * @param version     semantic version string
     */
    public PluginInfo(String name, String description, String version) {
        this(name, description, version, "action", "none");
    }

    /**
     * Create metadata for a tool-window plugin with a default zone.
     *
     * <p>Equivalent to
     * {@code new PluginInfo(name, description, version, "tool_window", defaultZone)}.</p>
     *
     * @param name        display name
     * @param description short description
     * @param version     semantic version string
     * @param defaultZone {@code "left"}, {@code "right"}, or {@code "bottom"}
     * @return a new PluginInfo configured as a tool-window plugin
     */
    public static PluginInfo toolWindow(String name, String description,
                                        String version, String defaultZone) {
        return new PluginInfo(name, description, version, "tool_window", defaultZone);
    }
}
