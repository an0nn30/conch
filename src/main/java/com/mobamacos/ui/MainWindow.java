package com.mobamacos.ui;

import com.mobamacos.config.ConfigManager;
import com.mobamacos.model.AppConfig;
import com.mobamacos.model.ServerEntry;
import com.mobamacos.model.ServerFolder;
import com.mobamacos.plugin.PluginContext;
import com.mobamacos.plugin.PluginManager;
import com.mobamacos.ssh.TunnelManager;
import com.mobamacos.theme.ThemeManager;
import com.mobamacos.ui.dialogs.NewConnectionDialog;
import com.mobamacos.ui.dialogs.PreferencesDialog;
import com.mobamacos.ui.dialogs.ResumeSessionsDialog;
import com.mobamacos.ui.files.FileTransferPanel;

import java.nio.file.Paths;

import javax.swing.*;
import java.awt.*;
import java.awt.event.WindowAdapter;
import java.awt.event.WindowEvent;
import java.util.ArrayList;
import java.util.List;

public class MainWindow extends JFrame {

    private final ConfigManager     configManager;
    private final ThemeManager      themeManager;
    private final SidePanel         sidePanel;       // left  — Files / Tools / Macros
    private final SessionTabPane    sessionTabPane;  // centre — terminals
    private final SessionsPanel     sessionsPanel;   // right  — server tree
    private final FileTransferPanel filePanel;

    private JSplitPane mainSplit;   // sidePanel | rightSplit
    private JSplitPane rightSplit;  // sessionTabPane | sessionsPanel

    public MainWindow(ConfigManager configManager, ThemeManager themeManager) {
        super("MobaMacOS Terminal");
        this.configManager = configManager;
        this.themeManager  = themeManager;

        setDefaultCloseOperation(DO_NOTHING_ON_CLOSE);
        setSize(1400, 900);
        setLocationRelativeTo(null);

        // Build sub-components
        sessionTabPane = new SessionTabPane(configManager);
        filePanel      = new FileTransferPanel();
        sessionTabPane.addSessionListener(filePanel);

        java.nio.file.Path configDir = Paths.get(System.getProperty("user.home"), ".mobamacos");
        PluginManager  pluginManager  = new PluginManager(configDir);
        PluginContext  pluginContext   = new PluginContext(sessionTabPane,
                sessionTabPane.getSshManager(), configManager);
        ToolsPanel     toolsPanel     = new ToolsPanel(pluginContext, pluginManager, configDir);

        sidePanel    = new SidePanel(filePanel, toolsPanel);
        sessionsPanel = new SessionsPanel(configManager, sessionTabPane);

        // Menu bar
        TunnelManager tunnelManager = new TunnelManager();
        MenuBarBuilder menuBuilder = new MenuBarBuilder(this, configManager, themeManager, tunnelManager);
        menuBuilder.setSessionsPanel(sessionsPanel);
        setJMenuBar(menuBuilder.build());


        // Layout: [left panel] | [terminals] | [sessions]
        rightSplit = new JSplitPane(JSplitPane.HORIZONTAL_SPLIT, sessionTabPane, sessionsPanel);
        rightSplit.setResizeWeight(1.0);          // terminals take all extra space
        rightSplit.setDividerSize(6);
        rightSplit.setContinuousLayout(true);
        rightSplit.setOneTouchExpandable(true);
        rightSplit.setDividerLocation(1400 - 240 - 260 - 6); // right panel starts ~240 from edge

        mainSplit = new JSplitPane(JSplitPane.HORIZONTAL_SPLIT, sidePanel, rightSplit);
        mainSplit.setDividerLocation(260);
        mainSplit.setDividerSize(6);
        mainSplit.setContinuousLayout(true);
        mainSplit.setOneTouchExpandable(true);

        add(mainSplit, BorderLayout.CENTER);

        addWindowListener(new WindowAdapter() {
            @Override public void windowClosing(WindowEvent e) { saveSessionsAndExit(); }
        });
    }

    /** Must be called after the window is visible. */
    public void handleStartup() {
        // Restore panel layout after the window has been laid out
        SwingUtilities.invokeLater(this::restorePanelLayout);

        AppConfig cfg = configManager.getConfig();
        List<String> lastKeys = cfg.getLastSessionKeys();

        if (!lastKeys.isEmpty() && !cfg.isDontAskResume()) {
            ResumeSessionsDialog dlg = new ResumeSessionsDialog(this, lastKeys);
            dlg.setVisible(true);

            if (dlg.isDontAskAgain()) {
                cfg.setDontAskResume(true);
                configManager.saveConfig();
            }

            if (dlg.isConfirmed()) {
                List<String> selected = dlg.getSelectedKeys();
                resumeSessions(selected);
                if (!selected.isEmpty()) return;
            }
        } else if (!lastKeys.isEmpty() && cfg.isDontAskResume()) {
            resumeSessions(lastKeys);
            if (!lastKeys.isEmpty()) return;
        }

        sessionTabPane.openLocalTerminal();
    }

    // -----------------------------------------------------------------------

    private void resumeSessions(List<String> keys) {
        for (String key : keys) {
            ServerEntry entry = findServerByKey(key);
            if (entry != null) sessionTabPane.openSession(entry);
        }
        if (sessionTabPane.getLiveSessionKeys().isEmpty() && tabbedPaneIsEmpty()) {
            sessionTabPane.openLocalTerminal();
        }
    }

    private boolean tabbedPaneIsEmpty() {
        for (Component c : sessionTabPane.getComponents()) {
            if (c instanceof JTabbedPane tp) return tp.getTabCount() == 0;
        }
        return true;
    }

    private ServerEntry findServerByKey(String key) {
        for (ServerFolder folder : configManager.getConfig().getFolders()) {
            for (ServerEntry server : folder.getServers()) {
                String k = server.getUsername() + "@" + server.getHost() + ":" + server.getPort();
                if (k.equals(key)) return server;
            }
        }
        return null;
    }

    private void saveSessionsAndExit() {
        List<String> keys = new ArrayList<>(sessionTabPane.getLiveSessionKeys());
        configManager.getConfig().setLastSessionKeys(keys);
        savePanelLayout();
        configManager.saveConfig();
        filePanel.dispose();
        System.exit(0);
    }

    // -----------------------------------------------------------------------
    // Panel layout persistence
    // -----------------------------------------------------------------------

    private void restorePanelLayout() {
        AppConfig cfg = configManager.getConfig();
        if (cfg.isLeftPanelCollapsed()) {
            mainSplit.setDividerLocation(0);
        } else {
            mainSplit.setDividerLocation(cfg.getLeftPanelWidth());
        }
        if (cfg.isRightPanelCollapsed()) {
            rightSplit.setDividerLocation(rightSplit.getWidth() - rightSplit.getDividerSize());
        } else {
            int loc = rightSplit.getWidth() - cfg.getRightPanelWidth() - rightSplit.getDividerSize();
            rightSplit.setDividerLocation(Math.max(0, loc));
        }
    }

    private void savePanelLayout() {
        AppConfig cfg = configManager.getConfig();
        int leftLoc = mainSplit.getDividerLocation();
        if (leftLoc > 5) {
            cfg.setLeftPanelWidth(leftLoc);
            cfg.setLeftPanelCollapsed(false);
        } else {
            cfg.setLeftPanelCollapsed(true);
        }
        int rightLoc   = rightSplit.getDividerLocation();
        int rightWidth = rightSplit.getWidth() - rightLoc - rightSplit.getDividerSize();
        if (rightWidth > 5) {
            cfg.setRightPanelWidth(rightWidth);
            cfg.setRightPanelCollapsed(false);
        } else {
            cfg.setRightPanelCollapsed(true);
        }
    }

    // -----------------------------------------------------------------------
    // macOS transparent title bar
    // -----------------------------------------------------------------------

    /**
     * Enables the macOS transparent/unified title bar.
     *
     * With {@code fullWindowContent=true} the root pane starts at y=0 (behind the
     * native title bar), so the JMenuBar — which the root-pane layout places first —
     * naturally occupies the title-bar area.  We just need to:
     *   1. Add a left inset to clear the traffic-light buttons.
     *   2. Push a couple of quick-action buttons to the far right via glue.
     */
    private void setupTransparentTitleBar() {
        getRootPane().putClientProperty("apple.awt.transparentTitleBar", Boolean.TRUE);
        getRootPane().putClientProperty("apple.awt.fullWindowContent",   Boolean.TRUE);

        JMenuBar menuBar = getJMenuBar();
        if (menuBar == null) return;

        // Left inset clears the traffic-light buttons (close/minimise/zoom ~70 px wide)
        menuBar.setBorder(BorderFactory.createEmptyBorder(0, 72, 0, 8));

        // Glue pushes everything added after this point to the far right
        menuBar.add(Box.createHorizontalGlue());
        menuBar.add(titleBarButton("+",  "New SSH Connection (⌘N)", this::showNewConnectionDialog));
        menuBar.add(titleBarButton("⚙", "Preferences (⌘,)",        this::showPreferencesDialog));
        menuBar.add(Box.createHorizontalStrut(8));
    }

    private static JButton titleBarButton(String text, String tooltip, Runnable action) {
        JButton b = new JButton(text);
        b.setToolTipText(tooltip);
        b.putClientProperty("JButton.buttonType", "roundRect");
        b.setFont(b.getFont().deriveFont(11f));
        b.setFocusPainted(false);
        b.setMargin(new Insets(1, 7, 1, 7));
        b.addActionListener(e -> action.run());
        return b;
    }

    private void showNewConnectionDialog() {
        NewConnectionDialog dlg = new NewConnectionDialog(this, configManager);
        dlg.addSavedListener(sessionsPanel::refresh);
        dlg.setConnectCallback(sessionTabPane::openSession);
        dlg.setVisible(true);
    }

    private void showPreferencesDialog() {
        new PreferencesDialog(this, configManager, themeManager).setVisible(true);
    }

    // -----------------------------------------------------------------------

    public SidePanel      getSidePanel()      { return sidePanel; }
    public SessionsPanel  getSessionsPanel()  { return sessionsPanel; }
    public SessionTabPane getSessionTabPane() { return sessionTabPane; }
}
