package com.mobamacos.plugin.api;

import com.mobamacos.config.ConfigManager;
import com.mobamacos.model.ServerEntry;
import com.mobamacos.model.ServerFolder;
import com.mobamacos.ui.SessionTabPane;

import javax.swing.*;
import java.awt.*;
import java.awt.datatransfer.StringSelection;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

/**
 * The {@code app} binding object available in every plugin script.
 */
public class PluginApp {

    private final SessionTabPane tabPane;
    private final ConfigManager  configManager;

    public PluginApp(SessionTabPane tabPane, ConfigManager configManager) {
        this.tabPane       = tabPane;
        this.configManager = configManager;
    }

    // -----------------------------------------------------------------------
    // Sessions
    // -----------------------------------------------------------------------

    /** Open a new SSH tab by saved-server name. */
    public void openSession(String name) {
        ServerEntry server = findSaved(name);
        if (server == null) throw new IllegalArgumentException("No saved server: " + name);
        SwingUtilities.invokeLater(() -> tabPane.openSession(server));
    }

    /** Open a new SSH tab by explicit parameters: {@code app.openSession(host:"1.2.3.4", user:"ubuntu")} */
    public void openSession(Map<String, Object> params) {
        ServerEntry server = new ServerEntry(
                str(params, "name", str(params, "host", "adhoc")),
                str(params, "host", "localhost"),
                num(params, "port", 22),
                str(params, "user", System.getProperty("user.name")));
        String key = (String) params.get("privateKey");
        if (key != null) server.setPrivateKeyPath(key);
        SwingUtilities.invokeLater(() -> tabPane.openSession(server));
    }

    // -----------------------------------------------------------------------
    // Clipboard
    // -----------------------------------------------------------------------

    /** Copy text to the system clipboard. */
    public void clipboard(String text) {
        SwingUtilities.invokeLater(() ->
            Toolkit.getDefaultToolkit().getSystemClipboard()
                   .setContents(new StringSelection(text), null));
    }

    // -----------------------------------------------------------------------
    // Notifications
    // -----------------------------------------------------------------------

    /** Brief non-blocking toast notification (uses system tray if available, else log). */
    public void notify(String message) {
        try {
            if (SystemTray.isSupported()) {
                SystemTray tray = SystemTray.getSystemTray();
                if (tray.getTrayIcons().length > 0) {
                    tray.getTrayIcons()[0].displayMessage(
                            "MobaMacOS", message, TrayIcon.MessageType.INFO);
                    return;
                }
            }
        } catch (Exception ignored) {}
        log(message);
    }

    // -----------------------------------------------------------------------
    // Config / info
    // -----------------------------------------------------------------------

    /** Returns a list of all saved server names. */
    public List<String> servers() {
        return configManager.getConfig().getFolders().stream()
                .flatMap(f -> f.getServers().stream())
                .map(ServerEntry::getName)
                .toList();
    }

    /** Write a message to stderr (visible in the IDE / console during development). */
    public void log(String message) {
        System.err.println("[plugin] " + message);
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    private ServerEntry findSaved(String name) {
        return configManager.getConfig().getFolders().stream()
                .flatMap(f -> f.getServers().stream())
                .filter(s -> name.equalsIgnoreCase(s.getName()))
                .findFirst().orElse(null);
    }

    private static String str(Map<String, Object> m, String k, String def) {
        Object v = m.get(k);
        return v != null ? v.toString() : def;
    }
    private static int num(Map<String, Object> m, String k, int def) {
        Object v = m.get(k);
        return v instanceof Number n ? n.intValue() : def;
    }
}
