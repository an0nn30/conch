package com.mobamacos.plugin.api;

import com.mobamacos.config.ConfigManager;
import com.mobamacos.model.ServerEntry;
import com.mobamacos.model.ServerFolder;
import com.mobamacos.ssh.SshSessionManager;
import com.mobamacos.ui.SessionTabPane;
import groovy.lang.Closure;
import net.schmizz.sshj.SSHClient;

import java.util.*;
import java.util.concurrent.*;

/**
 * The {@code session} binding object available in every plugin script.
 *
 * Groovy property access ({@code session.current}) maps to {@code getCurrent()}.
 */
public class PluginSession {

    private final SessionTabPane    tabPane;
    private final SshSessionManager sshManager;
    private final ConfigManager     configManager;

    public PluginSession(SessionTabPane tabPane, SshSessionManager sshManager,
                         ConfigManager configManager) {
        this.tabPane       = tabPane;
        this.sshManager    = sshManager;
        this.configManager = configManager;
    }

    // -----------------------------------------------------------------------
    // Session access (Groovy property syntax)
    // -----------------------------------------------------------------------

    /** The currently active session tab. */
    public SessionHandle getCurrent() { return tabPane.getActiveSessionHandle(); }

    /** The local shell session. */
    public SessionHandle getLocal()   { return tabPane.getLocalSessionHandle(); }

    /** All open session handles. */
    public List<SessionHandle> getAll() { return tabPane.getAllSessionHandles(); }

    /** Find an open session by saved-server name. Returns null if not found. */
    public SessionHandle named(String name) {
        return getAll().stream()
                .filter(h -> name.equalsIgnoreCase(h.getName()))
                .findFirst().orElse(null);
    }

    // -----------------------------------------------------------------------
    // Open a new session
    // -----------------------------------------------------------------------

    /**
     * Opens a new SSH tab to a saved server by name and returns its handle.
     * Blocks until the connection is established (or throws on failure).
     */
    public SessionHandle open(String serverName) throws Exception {
        ServerEntry server = findSaved(serverName);
        if (server == null)
            throw new IllegalArgumentException("No saved server named: " + serverName);
        return connectDetached(server);
    }

    /** Open by explicit parameters: {@code session.open(host:"1.2.3.4", user:"ubuntu", port:22)} */
    public SessionHandle open(Map<String, Object> params) throws Exception {
        ServerEntry server = new ServerEntry(
                str(params, "name", str(params, "host", "adhoc")),
                str(params, "host", "localhost"),
                num(params, "port", 22),
                str(params, "user", System.getProperty("user.name")));
        String key = (String) params.get("privateKey");
        if (key != null) server.setPrivateKeyPath(key);
        return connectDetached(server);
    }

    // -----------------------------------------------------------------------
    // Multi-session helpers
    // -----------------------------------------------------------------------

    /**
     * Runs {@code work} against each named session concurrently.
     * The closure receives a {@link SessionHandle} and should return a value.
     * Returns a {@code Map<String, Object>} of serverName → closure return value.
     *
     * <pre>{@code
     * def results = session.onAll(["web-01","web-02"]) { s -> s.exec("uptime") }
     * results.each { name, r -> println "$name: $r" }
     * }</pre>
     */
    public Map<String, Object> onAll(List<String> serverNames, Closure<?> work) throws Exception {
        ExecutorService pool = Executors.newCachedThreadPool();
        Map<String, Future<Object>> futures = new LinkedHashMap<>();
        for (String name : serverNames) {
            futures.put(name, pool.submit(() -> {
                SessionHandle h = resolveOrConnect(name);
                return work.call(h);
            }));
        }
        pool.shutdown();

        Map<String, Object> results = new LinkedHashMap<>();
        for (var entry : futures.entrySet()) {
            try {
                results.put(entry.getKey(), entry.getValue().get(60, TimeUnit.SECONDS));
            } catch (Exception e) {
                results.put(entry.getKey(), "ERROR: " + e.getMessage());
            }
        }
        return results;
    }

    /**
     * Runs {@code work} against each named session sequentially (opens+closes on demand).
     *
     * <pre>{@code
     * session.withEach(["db-01","db-02"]) { s ->
     *     println "${s.name}: ${s.exec('df -h /data | tail -1')}"
     * }
     * }</pre>
     */
    public void withEach(List<String> serverNames, Closure<?> work) throws Exception {
        for (String name : serverNames) {
            SessionHandle h = resolveOrConnect(name);
            work.call(h);
        }
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    private SessionHandle resolveOrConnect(String name) throws Exception {
        SessionHandle existing = named(name);
        if (existing != null) return existing;
        return open(name);
    }

    private SessionHandle connectDetached(ServerEntry server) throws Exception {
        // Authenticate without opening a PTY shell — plugins exec individual commands.
        // The SSHClient is owned by the SessionHandle; close it when done.
        SSHClient ssh = sshManager.connectAndAuth(server);
        return new SessionHandle(server.getName(), server.getHost(),
                server.getUsername(), null, ssh, null);
    }

    private ServerEntry findSaved(String name) {
        for (ServerFolder folder : configManager.getConfig().getFolders())
            for (ServerEntry s : folder.getServers())
                if (name.equalsIgnoreCase(s.getName())) return s;
        return null;
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
