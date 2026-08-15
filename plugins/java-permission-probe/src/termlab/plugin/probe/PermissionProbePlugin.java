package termlab.plugin.probe;

import termlab.plugin.TermLabPlugin;
import termlab.plugin.HostApi;
import termlab.plugin.PluginInfo;

/**
 * Attempts HostApi operations without declaring matching capabilities.
 * Expected behavior: calls are denied by host permission gates.
 */
public class PermissionProbePlugin implements TermLabPlugin {
    private static final String ACTION = "perm_probe.java.run";

    @Override
    public PluginInfo getInfo() {
        return new PluginInfo(
            "Java Permission Probe",
            "Attempts forbidden HostApi calls to validate enforcement",
            "0.1.0",
            "action",
            "none"
        );
    }

    @Override
    public void setup() {
        HostApi.registerMenuItem("Tools", "Permissions: Run Java Probe", ACTION);
        HostApi.info("Java Permission Probe loaded");
    }

    @Override
    public void onEvent(String eventJson) {
        if (!eventJson.contains(ACTION)) {
            return;
        }

        String clip = HostApi.clipboardGet();
        HostApi.clipboardSet("java-probe-write");
        HostApi.setConfig("java_permission_probe", "blocked");

        String body =
            "clipboardGet=" + (clip == null ? "null" : "value returned") + "\n" +
            "clipboardSet attempted\n" +
            "setConfig attempted\n" +
            "If enforcement is active, denied-warning toasts should appear.";
        HostApi.notify("Java Permission Probe", body, "info", 5000);
    }

    @Override
    public String render() {
        return "[]";
    }

    @Override
    public void teardown() {
        HostApi.info("Java Permission Probe unloaded");
    }
}
