package com.mobamacos.ui;

import com.jediterm.terminal.ui.JediTermWidget;
import com.mobamacos.config.ConfigManager;
import com.mobamacos.model.ServerEntry;
import com.mobamacos.plugin.api.SessionHandle;
import com.mobamacos.ssh.MoshTtyConnector;
import com.mobamacos.ssh.MoshTtyConnector.CwdListener;
import com.mobamacos.ssh.SshjTtyConnector;
import com.mobamacos.ssh.SshSessionManager;
import com.mobamacos.terminal.LocalShellTtyConnector;
import com.mobamacos.terminal.TerminalSettingsProvider;
import net.schmizz.sshj.SSHClient;

import javax.swing.*;
import javax.swing.SwingUtilities;
import java.awt.*;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public class SessionTabPane extends JPanel {

    private final ConfigManager     configManager;
    private final SshSessionManager sshManager;
    private final JTabbedPane       tabbedPane;

    /** Tracks which JediTermWidget corresponds to which ServerEntry (SSH/Mosh sessions). */
    private final Map<JediTermWidget, ServerEntry>       sessionMap        = new LinkedHashMap<>();
    /** Tracks which JediTermWidget corresponds to which SshjTtyConnector. */
    private final Map<JediTermWidget, SshjTtyConnector> connectorMap      = new LinkedHashMap<>();
    /** Tracks which JediTermWidget corresponds to which MoshTtyConnector (for CWD tracking). */
    private final Map<JediTermWidget, MoshTtyConnector>       moshConnectorMap  = new LinkedHashMap<>();
    /** Tracks which JediTermWidget corresponds to which LocalShellTtyConnector (for CWD tracking). */
    private final Map<JediTermWidget, LocalShellTtyConnector> localConnectorMap = new LinkedHashMap<>();

    /** Listeners notified when the active SSH session changes (e.g. FileTransferPanel). */
    private final List<SessionListener> sessionListeners = new ArrayList<>();

    public interface SessionListener {
        void onSessionActivated(ServerEntry server);
        /** Called (on the EDT) when the remote shell reports a new working directory. */
        default void onCwdChanged(String absolutePath) {}
        /** Called (on the EDT) when a local terminal tab becomes active. */
        default void onLocalSessionActivated() {}
        void onSessionDeactivated();
    }

    public void addSessionListener(SessionListener l) { sessionListeners.add(l); }

    public SessionTabPane(ConfigManager configManager) {
        this.configManager = configManager;
        this.sshManager    = new SshSessionManager();

        setLayout(new BorderLayout());

        tabbedPane = new JTabbedPane(JTabbedPane.TOP, JTabbedPane.SCROLL_TAB_LAYOUT);
        syncTabBackground();
        add(tabbedPane, BorderLayout.CENTER);

        tabbedPane.addChangeListener(e -> {
            int idx = tabbedPane.getSelectedIndex();
            if (idx >= 0) {
                Component comp = tabbedPane.getComponentAt(idx);
                ServerEntry server = sessionMap.get(comp);
                if (server != null) {
                    // Wire CWD listener for the newly-active SSH connector
                    SshjTtyConnector conn = connectorMap.get(comp);
                    if (conn != null) {
                        conn.setCwdListener(path ->
                            SwingUtilities.invokeLater(() ->
                                sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    }
                    // Wire CWD listener for the newly-active Mosh connector
                    MoshTtyConnector moshConn = moshConnectorMap.get(comp);
                    if (moshConn != null) {
                        moshConn.setCwdListener(path ->
                            SwingUtilities.invokeLater(() ->
                                sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    }
                    sessionListeners.forEach(l -> l.onSessionActivated(server));
                    return;
                }
                // Wire CWD listener for the newly-active local connector
                LocalShellTtyConnector localConn = localConnectorMap.get(comp);
                if (localConn != null) {
                    localConn.setCwdListener(path ->
                        SwingUtilities.invokeLater(() ->
                            sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    sessionListeners.forEach(SessionListener::onLocalSessionActivated);
                    return;
                }
            }
            // No session active — clear CWD listeners on all connectors
            connectorMap.values().forEach(c -> c.setCwdListener(null));
            moshConnectorMap.values().forEach(c -> c.setCwdListener(null));
            localConnectorMap.values().forEach(c -> c.setCwdListener(null));
            sessionListeners.forEach(SessionListener::onSessionDeactivated);
        });
    }

    /**
     * Called by Swing whenever the Look & Feel changes (via updateComponentTreeUI).
     * Explicitly re-syncs the tab-area background so FlatLaf dark themes don't
     * leave a stale light background from the previous L&F.
     */
    @Override
    public void updateUI() {
        super.updateUI();
        syncTabBackground();
    }

    private void syncTabBackground() {
        if (tabbedPane == null) return;
        Color bg = UIManager.getColor("TabbedPane.background");
        if (bg != null) tabbedPane.setBackground(bg);
    }

    /** Exposes the SSH manager so FileTransferPanel can create SFTP connections. */
    public SshSessionManager getSshManager() { return sshManager; }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /** Opens a local PTY shell tab. Safe to call on EDT. */
    public void openLocalTerminal() {
        SwingWorker<LocalShellTtyConnector, Void> worker = new SwingWorker<>() {
            @Override
            protected LocalShellTtyConnector doInBackground() throws Exception {
                return new LocalShellTtyConnector();
            }

            @Override
            protected void done() {
                try {
                    LocalShellTtyConnector connector = get();
                    JediTermWidget terminal = createTerminalWidget();
                    terminal.createTerminalSession(connector);
                    terminal.start();

                    localConnectorMap.put(terminal, connector);

                    String title = "Local Terminal";
                    int idx = tabbedPane.getTabCount();
                    tabbedPane.addTab(title, terminal);
                    tabbedPane.setSelectedIndex(idx);
                    tabbedPane.setTabComponentAt(idx, new TabHeader(tabbedPane, title, () -> {
                        localConnectorMap.remove(terminal);
                        connector.setCwdListener(null);
                        try { connector.close(); } catch (Exception ignored) {}
                    }));

                    // Wire CWD listener immediately (this tab is now selected)
                    connector.setCwdListener(path ->
                        SwingUtilities.invokeLater(() ->
                            sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    sessionListeners.forEach(SessionListener::onLocalSessionActivated);

                    watchForExit(connector, terminal, null);
                    terminal.requestFocusInWindow();
                } catch (Exception e) {
                    JOptionPane.showMessageDialog(SessionTabPane.this,
                            "Could not open local terminal:\n" + e.getMessage(),
                            "Error", JOptionPane.ERROR_MESSAGE);
                }
            }
        };
        worker.execute();
    }

    /** Opens an SSH or Mosh session tab. */
    public void openSession(ServerEntry server) {
        if (server.isUseMosh()) { openMoshSession(server); return; }
        openSshSession(server);
    }

    private void openSshSession(ServerEntry server) {
        String title = server.getName();
        JPanel placeholder = buildConnectingPanel(server);
        int idx = tabbedPane.getTabCount();
        tabbedPane.addTab(title, placeholder);
        tabbedPane.setSelectedIndex(idx);
        TabHeader header = new TabHeader(tabbedPane, title, null);
        tabbedPane.setTabComponentAt(idx, header);

        SwingWorker<SshjTtyConnector, Void> worker = new SwingWorker<>() {
            @Override protected SshjTtyConnector doInBackground() throws Exception {
                return sshManager.connect(server);
            }

            @Override
            protected void done() {
                int currentIdx = tabbedPane.indexOfComponent(placeholder);
                try {
                    SshjTtyConnector connector = get();
                    if (currentIdx < 0) {
                        try { connector.close(); } catch (Exception ignored) {}
                        return;
                    }

                    JediTermWidget terminal = createTerminalWidget();
                    terminal.createTerminalSession(connector);
                    terminal.start();

                    sessionMap.put(terminal, server);
                    connectorMap.put(terminal, connector);

                    // Wire CWD listener immediately if this is the selected tab
                    if (tabbedPane.getSelectedComponent() == placeholder
                            || tabbedPane.getSelectedIndex() == currentIdx) {
                        connector.setCwdListener(path ->
                            SwingUtilities.invokeLater(() ->
                                sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    }

                    tabbedPane.setComponentAt(currentIdx, terminal);

                    // setComponentAt does NOT fire the ChangeListener (index unchanged),
                    // so explicitly notify if this tab is currently selected.
                    if (tabbedPane.getSelectedIndex() == currentIdx) {
                        sessionListeners.forEach(l -> l.onSessionActivated(server));
                    }

                    header.setCloseAction(() -> {
                        sessionMap.remove(terminal);
                        connectorMap.remove(terminal);
                        connector.setCwdListener(null);
                        try { terminal.close(); } catch (Exception ignored) {}
                    });
                    tabbedPane.revalidate();
                    tabbedPane.repaint();
                    terminal.requestFocusInWindow();

                    watchForExit(connector, terminal, server);

                } catch (Exception e) {
                    if (currentIdx < 0) return;
                    Throwable cause = e.getCause() != null ? e.getCause() : e;
                    tabbedPane.setComponentAt(currentIdx, buildErrorPanel(cause.getMessage(), server));
                    tabbedPane.revalidate();
                    tabbedPane.repaint();
                }
            }
        };
        worker.execute();
    }

    private void openMoshSession(ServerEntry server) {
        String title = server.getName() + " [mosh]";
        JPanel placeholder = buildConnectingPanel(server);
        int idx = tabbedPane.getTabCount();
        tabbedPane.addTab(title, placeholder);
        tabbedPane.setSelectedIndex(idx);
        TabHeader header = new TabHeader(tabbedPane, title, null);
        tabbedPane.setTabComponentAt(idx, header);

        SwingWorker<MoshTtyConnector, Void> worker = new SwingWorker<>() {
            @Override protected MoshTtyConnector doInBackground() throws Exception {
                return new MoshTtyConnector(server);
            }

            @Override
            protected void done() {
                int currentIdx = tabbedPane.indexOfComponent(placeholder);
                try {
                    MoshTtyConnector connector = get();
                    if (currentIdx < 0) { connector.close(); return; }

                    JediTermWidget terminal = createTerminalWidget();
                    terminal.createTerminalSession(connector);
                    terminal.start();

                    sessionMap.put(terminal, server);
                    moshConnectorMap.put(terminal, connector);

                    // Wire CWD listener immediately if this is the selected tab
                    if (tabbedPane.getSelectedComponent() == placeholder
                            || tabbedPane.getSelectedIndex() == currentIdx) {
                        connector.setCwdListener(path ->
                            SwingUtilities.invokeLater(() ->
                                sessionListeners.forEach(l -> l.onCwdChanged(path))));
                    }

                    tabbedPane.setComponentAt(currentIdx, terminal);

                    if (tabbedPane.getSelectedIndex() == currentIdx) {
                        sessionListeners.forEach(l -> l.onSessionActivated(server));
                    }

                    header.setCloseAction(() -> {
                        sessionMap.remove(terminal);
                        moshConnectorMap.remove(terminal);
                        connector.setCwdListener(null);
                        try { terminal.close(); } catch (Exception ignored) {}
                        connector.close();
                    });
                    tabbedPane.revalidate();
                    tabbedPane.repaint();
                    terminal.requestFocusInWindow();

                    // Mosh-specific exit watcher: keep the tab open on non-zero exit
                    // so the user can read the error output in the terminal.
                    Thread watcher = new Thread(() -> {
                        int code;
                        try { code = connector.waitFor(); } catch (InterruptedException ex) { return; }
                        SwingUtilities.invokeLater(() -> {
                            sessionMap.remove(terminal);
                            moshConnectorMap.remove(terminal);
                            int i = tabbedPane.indexOfComponent(terminal);
                            if (i < 0) return;
                            if (code == 0) {
                                tabbedPane.remove(i);
                            } else {
                                // Leave the tab open so the error output is readable
                                header.setTitle("[disconnected] " + server.getName());
                            }
                        });
                    }, "mosh-exit-watcher");
                    watcher.setDaemon(true);
                    watcher.start();

                } catch (Exception e) {
                    if (currentIdx < 0) return;
                    Throwable cause = e.getCause() != null ? e.getCause() : e;
                    tabbedPane.setComponentAt(currentIdx, buildErrorPanel(cause.getMessage(), server));
                    tabbedPane.revalidate();
                    tabbedPane.repaint();
                }
            }
        };
        worker.execute();
    }

    // -----------------------------------------------------------------------
    // Plugin API: session handle access
    // -----------------------------------------------------------------------

    /** Returns a {@link SessionHandle} for the currently active tab, or {@code null}. */
    public SessionHandle getActiveSessionHandle() {
        Component comp = tabbedPane.getSelectedComponent();
        if (comp instanceof JediTermWidget w) return buildHandle(w);
        return null;
    }

    /** Returns {@link SessionHandle}s for every open tab. */
    public List<SessionHandle> getAllSessionHandles() {
        List<SessionHandle> handles = new ArrayList<>();
        for (int i = 0; i < tabbedPane.getTabCount(); i++) {
            Component comp = tabbedPane.getComponentAt(i);
            if (comp instanceof JediTermWidget w) {
                SessionHandle h = buildHandle(w);
                if (h != null) handles.add(h);
            }
        }
        return handles;
    }

    /** Returns a {@link SessionHandle} for the local terminal, or {@code null}. */
    public SessionHandle getLocalSessionHandle() {
        for (LocalShellTtyConnector c : localConnectorMap.values()) {
            return new SessionHandle("local",
                    text -> { try { c.write(text); } catch (Exception ignored) {} });
        }
        return null;
    }

    private SessionHandle buildHandle(JediTermWidget w) {
        ServerEntry server = sessionMap.get(w);
        if (server != null) {
            SshjTtyConnector conn = connectorMap.get(w);
            SSHClient sshClient = conn != null ? conn.getSshClient() : null;
            return new SessionHandle(server.getName(), server.getHost(),
                    server.getUsername(), null, sshClient,
                    text -> { try { if (conn != null) conn.write(text); } catch (Exception ignored) {} });
        }
        LocalShellTtyConnector local = localConnectorMap.get(w);
        if (local != null) {
            return new SessionHandle("local",
                    text -> { try { local.write(text); } catch (Exception ignored) {} });
        }
        return null;
    }

    /**
     * Returns session keys ("user@host:port") for all currently-live SSH tabs.
     */
    public List<String> getLiveSessionKeys() {
        List<String> keys = new ArrayList<>();
        for (Map.Entry<JediTermWidget, ServerEntry> e : sessionMap.entrySet()) {
            if (e.getKey().isShowing()) {
                ServerEntry s = e.getValue();
                keys.add(s.getUsername() + "@" + s.getHost() + ":" + s.getPort());
            }
        }
        return keys;
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    private JediTermWidget createTerminalWidget() {
        return new JediTermWidget(new TerminalSettingsProvider(configManager.getConfig()));
    }

    private void watchForExit(com.jediterm.terminal.TtyConnector connector,
                               JediTermWidget terminal, ServerEntry server) {
        Thread watcher = new Thread(() -> {
            try { connector.waitFor(); } catch (InterruptedException ignored) {}
            SwingUtilities.invokeLater(() -> {
                int i = tabbedPane.indexOfComponent(terminal);
                if (i >= 0) {
                    if (server != null) {
                        sessionMap.remove(terminal);
                        connectorMap.remove(terminal);
                    }
                    localConnectorMap.remove(terminal);
                    tabbedPane.remove(i);
                }
            });
        }, "exit-watcher");
        watcher.setDaemon(true);
        watcher.start();
    }

    private JPanel buildConnectingPanel(ServerEntry server) {
        JPanel p = new JPanel(new GridBagLayout());
        GridBagConstraints gbc = new GridBagConstraints();
        gbc.gridwidth = GridBagConstraints.REMAINDER;
        gbc.insets    = new Insets(8, 8, 8, 8);

        JLabel title = new JLabel("Connecting to " + server.getName() + "\u2026");
        title.setFont(title.getFont().deriveFont(Font.BOLD, 18f));
        p.add(title, gbc);

        JLabel host = new JLabel(server.getUsername() + "@" + server.getHost() + ":" + server.getPort());
        host.setFont(host.getFont().deriveFont(12f));
        p.add(host, gbc);

        p.add(Box.createVerticalStrut(8), gbc);

        JProgressBar bar = new JProgressBar();
        bar.setIndeterminate(true);
        bar.setPreferredSize(new Dimension(300, bar.getPreferredSize().height));
        p.add(bar, gbc);

        return p;
    }

    private JPanel buildErrorPanel(String message, ServerEntry server) {
        JPanel p = new JPanel(new GridBagLayout());
        GridBagConstraints gbc = new GridBagConstraints();
        gbc.gridwidth = GridBagConstraints.REMAINDER;
        gbc.insets    = new Insets(8, 8, 8, 8);

        JLabel title = new JLabel("Connection Failed");
        title.setFont(title.getFont().deriveFont(Font.BOLD, 18f));
        title.setForeground(Color.RED);
        p.add(title, gbc);

        JLabel err = new JLabel("<html><center>" + (message != null ? message : "") + "</center></html>");
        err.setFont(err.getFont().deriveFont(12f));
        p.add(err, gbc);
        p.add(Box.createVerticalStrut(8), gbc);

        JButton retry = new JButton("Retry");
        retry.addActionListener(e -> {
            int idx = tabbedPane.indexOfComponent(p);
            if (idx >= 0) tabbedPane.remove(idx);
            openSession(server);
        });
        p.add(retry, gbc);

        return p;
    }

    // -----------------------------------------------------------------------
    // Inner class: tab header with ×  close button
    // -----------------------------------------------------------------------

    static class TabHeader extends JPanel {

        private Runnable      closeAction;
        private final JLabel  titleLabel;

        TabHeader(JTabbedPane pane, String title, Runnable closeAction) {
            super(new FlowLayout(FlowLayout.LEFT, 0, 0));
            setOpaque(false);
            this.closeAction = closeAction;

            titleLabel = new JLabel(title);
            titleLabel.setBorder(BorderFactory.createEmptyBorder(0, 0, 0, 6));
            add(titleLabel);

            JButton close = new JButton("\u00d7");
            close.setFont(close.getFont().deriveFont(Font.BOLD, 13f));
            close.setMargin(new Insets(0, 2, 0, 2));
            close.setBorderPainted(false);
            close.setContentAreaFilled(false);
            close.setFocusPainted(false);
            close.setToolTipText("Close tab");
            close.addActionListener(e -> {
                int idx = pane.indexOfTabComponent(TabHeader.this);
                if (idx >= 0) {
                    if (this.closeAction != null) this.closeAction.run();
                    pane.remove(idx);
                }
            });
            add(close);
        }

        void setCloseAction(Runnable r) { this.closeAction = r; }
        void setTitle(String t)         { titleLabel.setText(t); }
    }
}
