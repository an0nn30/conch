package conch.plugin;

/**
 * Interface that all Conch Java plugins must implement.
 *
 * <p>The host discovers plugins by reading the {@code Plugin-Class} attribute
 * from the JAR's {@code META-INF/MANIFEST.MF}. The class named there must
 * implement this interface and have a public no-argument constructor.</p>
 *
 * <h2>Lifecycle</h2>
 * <ol>
 *   <li>The host instantiates the plugin class via its no-arg constructor.</li>
 *   <li>{@link #getInfo()} is called to read plugin metadata.</li>
 *   <li>{@link #setup()} is called once on a dedicated plugin thread.
 *       Use this to register menu items, subscribe to events, and
 *       initialize state.</li>
 *   <li>{@link #onEvent(String)} is called whenever a UI or bus event
 *       targets this plugin (menu clicks, widget interactions, etc.).</li>
 *   <li>{@link #render()} is called on every frame to get the current
 *       widget tree for tool-window plugins.</li>
 *   <li>{@link #teardown()} is called when the plugin is unloaded.
 *       Release resources here.</li>
 * </ol>
 *
 * <h2>Threading</h2>
 * <p>All methods are called on a single dedicated thread per plugin.
 * You do <b>not</b> need to synchronize internal state. However,
 * {@link HostApi} methods are safe to call from any thread.</p>
 *
 * <h2>Example</h2>
 * <pre>
 * public class MyPlugin implements ConchPlugin {
 *
 *     &#64;Override
 *     public PluginInfo getInfo() {
 *         return new PluginInfo("My Plugin", "Does cool things", "1.0.0");
 *     }
 *
 *     &#64;Override
 *     public void setup() {
 *         HostApi.registerMenuItem("Tools", "Do Thing", "do_thing");
 *     }
 *
 *     &#64;Override
 *     public void onEvent(String eventJson) {
 *         if (eventJson.contains("do_thing")) {
 *             HostApi.info("Thing was done!");
 *         }
 *     }
 *
 *     &#64;Override
 *     public String render() {
 *         return "[]"; // No panel widgets
 *     }
 *
 *     &#64;Override
 *     public void teardown() {}
 * }
 * </pre>
 *
 * @see HostApi
 * @see PluginInfo
 */
public interface ConchPlugin {

    /**
     * Return metadata describing this plugin.
     *
     * <p>Called once during plugin discovery and again when the plugin is
     * loaded. The returned info is displayed in the Plugin Manager UI.</p>
     *
     * @return a {@link PluginInfo} with the plugin's name, version, and type
     */
    PluginInfo getInfo();

    /**
     * Initialize the plugin.
     *
     * <p>Called once on the plugin's dedicated thread after instantiation.
     * This is the place to:</p>
     * <ul>
     *   <li>Register menu items via {@link HostApi#registerMenuItem}</li>
     *   <li>Set up internal state</li>
     *   <li>Log startup messages via {@link HostApi#info}</li>
     * </ul>
     */
    void setup();

    /**
     * Handle an incoming event.
     *
     * <p>Events are delivered as JSON strings. Common event types include:</p>
     * <ul>
     *   <li><b>Menu actions</b> — when the user clicks a registered menu item:
     *       {@code {"kind":"menu_action","action":"your_action"}}</li>
     *   <li><b>Widget events</b> — button clicks, text input changes, etc.:
     *       {@code {"kind":"widget","type":"button_click","id":"btn_id"}}</li>
     *   <li><b>Bus events</b> — inter-plugin communication:
     *       {@code {"kind":"bus_event","event_type":"ssh.connected","data":{...}}}</li>
     * </ul>
     *
     * <p>Use {@code String.contains()} for simple matching, or a JSON
     * library (Gson, Jackson, org.json) for structured parsing.</p>
     *
     * @param eventJson JSON-encoded event string
     */
    void onEvent(String eventJson);

    /**
     * Handle a direct RPC-style query from another plugin.
     *
     * <p>This is optional. The default implementation returns {@code "null"}.
     * If implemented, return a JSON value string (object/array/string/number/bool/null).</p>
     *
     * @param method query method name
     * @param argsJson JSON-encoded args payload
     * @return JSON-encoded result value string
     */
    default String onQuery(String method, String argsJson) {
        return "null";
    }

    /**
     * Return the current widget tree as a JSON array string.
     *
     * <p>Called on every UI frame for tool-window plugins. Return {@code "[]"} if
     * the plugin has no tool window or no widgets to display.</p>
     *
     * <p>Widget types include Button, Label, TextInput, TreeView, Toolbar,
     * Separator, and more. See the Conch Plugin SDK documentation for the
     * full widget JSON schema.</p>
     *
     * <p><b>Example:</b></p>
     * <pre>{@code
     * return "[{\"Button\":{\"id\":\"click_me\",\"label\":\"Click Me\",\"icon\":null,\"enabled\":null}}]";
     * }</pre>
     *
     * @return JSON array of widget objects, or {@code "[]"} for no widgets
     */
    String render();

    /**
     * Return the widget tree for a specific host view.
     *
     * <p>The host passes a view id (for example, a plugin-defined settings view).
     * Default implementation falls back to {@link #render()} for compatibility.</p>
     *
     * @param viewId host-defined view identifier
     * @return JSON array of widget objects
     */
    default String renderView(String viewId) {
        return render();
    }

    /**
     * Clean up before the plugin is unloaded.
     *
     * <p>Called once when the user unloads the plugin or the application
     * exits. Close open connections, flush buffers, and release resources
     * here.</p>
     */
    void teardown();
}
