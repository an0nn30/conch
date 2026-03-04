package com.github.an0nn30.conch.ui.dialogs;

import com.github.an0nn30.conch.config.ConfigManager;
import com.github.an0nn30.conch.model.SavedTunnel;
import com.github.an0nn30.conch.model.ServerEntry;
import com.github.an0nn30.conch.model.ServerFolder;
import com.github.an0nn30.conch.ssh.TunnelManager;

import javax.swing.*;
import javax.swing.table.DefaultTableCellRenderer;
import javax.swing.table.DefaultTableModel;
import java.awt.*;
import java.util.List;

public class TunnelManagerDialog extends JDialog {

    private final ConfigManager configManager;
    private final TunnelManager tunnelManager;

    private final DefaultTableModel tableModel;
    private final JTable            table;
    private       Timer             refreshTimer;

    private static final String[] COLUMNS =
            { "Status", "Label", "Local Port", "Remote", "Via" };

    public TunnelManagerDialog(Window owner,
                               ConfigManager configManager,
                               TunnelManager tunnelManager) {
        super(owner, "SSH Tunnels", ModalityType.MODELESS);
        this.configManager = configManager;
        this.tunnelManager = tunnelManager;

        setSize(660, 360);
        setMinimumSize(new Dimension(500, 260));
        setLocationRelativeTo(owner);

        tableModel = new DefaultTableModel(COLUMNS, 0) {
            @Override public boolean isCellEditable(int r, int c) { return false; }
        };
        table = buildTable();

        JScrollPane scroll = new JScrollPane(table);

        JButton newBtn        = new JButton("New Tunnel\u2026");
        JButton activateBtn   = new JButton("Activate");
        JButton deactivateBtn = new JButton("Deactivate");
        JButton deleteBtn     = new JButton("Delete");
        JButton closeBtn      = new JButton("Close");

        newBtn.addActionListener(e        -> openNewTunnelDialog());
        activateBtn.addActionListener(e   -> activateSelected());
        deactivateBtn.addActionListener(e -> deactivateSelected());
        deleteBtn.addActionListener(e     -> deleteSelected());
        closeBtn.addActionListener(e      -> dispose());

        JPanel buttons = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 8));
        buttons.add(newBtn);
        buttons.add(activateBtn);
        buttons.add(deactivateBtn);
        buttons.add(deleteBtn);
        buttons.add(closeBtn);

        JLabel hint = new JLabel(
                "  Tunnels forward localhost:localPort to remoteHost:remotePort via the SSH server.");
        hint.setFont(hint.getFont().deriveFont(Font.ITALIC, 11f));

        getContentPane().setLayout(new BorderLayout());
        getContentPane().add(hint,    BorderLayout.NORTH);
        getContentPane().add(scroll,  BorderLayout.CENTER);
        getContentPane().add(buttons, BorderLayout.SOUTH);

        refreshTable();

        // Refresh every 3 s so dropped tunnels are reflected in the Status column
        refreshTimer = new Timer(3000, e -> refreshTable());
        refreshTimer.start();
        addWindowListener(new java.awt.event.WindowAdapter() {
            @Override public void windowClosed(java.awt.event.WindowEvent e) {
                refreshTimer.stop();
            }
        });
    }

    // -----------------------------------------------------------------------

    private JTable buildTable() {
        JTable t = new JTable(tableModel);
        t.setSelectionMode(ListSelectionModel.SINGLE_SELECTION);
        t.setRowHeight(22);
        t.getTableHeader().setReorderingAllowed(false);

        // Column widths
        t.getColumnModel().getColumn(0).setPreferredWidth(80);
        t.getColumnModel().getColumn(0).setMaxWidth(90);
        t.getColumnModel().getColumn(2).setPreferredWidth(80);
        t.getColumnModel().getColumn(2).setMaxWidth(90);

        // Color the Status column
        t.getColumnModel().getColumn(0).setCellRenderer(new DefaultTableCellRenderer() {
            private static final Color GREEN  = new Color(60, 180, 60);
            private static final Color GRAY   = new Color(140, 140, 140);

            @Override
            public Component getTableCellRendererComponent(
                    JTable tbl, Object val, boolean sel, boolean focus, int row, int col) {
                super.getTableCellRendererComponent(tbl, val, sel, focus, row, col);
                boolean active = "Active".equals(val);
                if (!sel) setForeground(active ? GREEN : GRAY);
                setText(active ? "\u25cf Active" : "\u25cb Inactive");
                return this;
            }
        });

        return t;
    }

    // -----------------------------------------------------------------------

    private void refreshTable() {
        int selectedRow = table.getSelectedRow();
        tableModel.setRowCount(0);

        List<SavedTunnel> saved = configManager.getConfig().getTunnels();
        for (SavedTunnel st : saved) {
            tableModel.addRow(new Object[]{
                    tunnelManager.isActive(st) ? "Active" : "Inactive",
                    st.getLabel(),
                    st.getLocalPort(),
                    st.getRemoteHost() + ":" + st.getRemotePort(),
                    st.serverDisplayName()
            });
        }

        if (selectedRow >= 0 && selectedRow < tableModel.getRowCount()) {
            table.getSelectionModel().setSelectionInterval(selectedRow, selectedRow);
        }
    }

    private SavedTunnel selectedTunnel() {
        int row = table.getSelectedRow();
        if (row < 0) return null;
        List<SavedTunnel> saved = configManager.getConfig().getTunnels();
        return (row < saved.size()) ? saved.get(row) : null;
    }

    private void activateSelected() {
        SavedTunnel st = selectedTunnel();
        if (st == null || tunnelManager.isActive(st)) return;

        ServerEntry server = findServer(st);
        if (server == null) {
            JOptionPane.showMessageDialog(this,
                    "No matching server found for " + st.serverDisplayName()
                    + ".\nMake sure the server is configured in the sidebar.",
                    "Server Not Found", JOptionPane.WARNING_MESSAGE);
            return;
        }

        SwingWorker<Void, Void> worker = new SwingWorker<>() {
            @Override protected Void doInBackground() throws Exception {
                tunnelManager.activate(st, server);
                return null;
            }
            @Override protected void done() {
                try {
                    get();
                } catch (Exception e) {
                    Throwable cause = e.getCause() != null ? e.getCause() : e;
                    JOptionPane.showMessageDialog(TunnelManagerDialog.this,
                            "Failed to activate tunnel: " + cause.getMessage(),
                            "Tunnel Error", JOptionPane.ERROR_MESSAGE);
                }
                refreshTable();
            }
        };
        worker.execute();
    }

    private void deactivateSelected() {
        SavedTunnel st = selectedTunnel();
        if (st == null || !tunnelManager.isActive(st)) return;
        tunnelManager.deactivate(st);
        refreshTable();
    }

    private void deleteSelected() {
        SavedTunnel st = selectedTunnel();
        if (st == null) return;

        int choice = JOptionPane.showConfirmDialog(this,
                "Delete tunnel \"" + st.getLabel() + "\"?",
                "Confirm Delete", JOptionPane.OK_CANCEL_OPTION);
        if (choice != JOptionPane.OK_OPTION) return;

        tunnelManager.deactivate(st);
        configManager.getConfig().getTunnels().remove(st);
        configManager.saveConfig();
        refreshTable();
    }

    private ServerEntry findServer(SavedTunnel st) {
        for (ServerFolder f : configManager.getConfig().getFolders()) {
            for (ServerEntry s : f.getServers()) {
                if (s.getHost().equals(st.getServerHost())
                        && s.getPort() == st.getServerPort()
                        && s.getUsername().equals(st.getServerUsername())) {
                    return s;
                }
            }
        }
        return null;
    }

    private void openNewTunnelDialog() {
        NewTunnelDialog dlg = new NewTunnelDialog(this, configManager, tunnelManager);
        dlg.setOnConnected(this::refreshTable);
        dlg.setVisible(true);
    }
}
