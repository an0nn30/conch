package termlab.plugin.hello;

import termlab.plugin.TermLabPlugin;
import termlab.plugin.HostApi;
import termlab.plugin.PluginInfo;

/**
 * Java parity sample plugin for TermLab.
 *
 * Demonstrates:
 * - menu registration
 * - status/notification APIs
 * - active session introspection
 * - service registration + query RPC via onQuery()
 */
public class HelloPlugin implements TermLabPlugin {
    private static final String ACTION_HELLO = "say_hello";
    private static final String ACTION_QUERY = "query_self";
    private static final String SERVICE_NAME = "hello_java_service";

    @Override
    public PluginInfo getInfo() {
        return new PluginInfo(
            "Hello Java",
            "Test Java plugin - registers a menu item",
            "0.1.0",
            "action",
            "none"
        );
    }

    @Override
    public void setup() {
        HostApi.info("Hello Java plugin: setup");
        HostApi.registerMenuItem("Tools", "Java: Say Hello", ACTION_HELLO);
        HostApi.registerMenuItem("Tools", "Java: Query Service", ACTION_QUERY);
        HostApi.registerService(SERVICE_NAME);
        HostApi.setStatus("Hello Java ready", 0, -1.0f);
    }

    @Override
    public void onEvent(String eventJson) {
        if (eventJson.contains(ACTION_HELLO)) {
            String active = HostApi.getActiveSession();
            HostApi.info("Hello from Java plugin! Active session=" + active);
            HostApi.notify("Hello Java", "Active session: " + active, "info", 3500);
            return;
        }
        if (eventJson.contains(ACTION_QUERY)) {
            String result = HostApi.queryPlugin(SERVICE_NAME, "hello_status", "{}");
            HostApi.notify("Hello Java RPC", String.valueOf(result), "success", 3500);
        }
    }

    @Override
    public String onQuery(String method, String argsJson) {
        if ("hello_status".equals(method)) {
            return "{\"plugin\":\"Hello Java\",\"status\":\"ok\"}";
        }
        return "null";
    }

    @Override
    public String render() {
        return "[]";
    }

    @Override
    public void teardown() {
        HostApi.info("Hello Java plugin: teardown");
    }
}
