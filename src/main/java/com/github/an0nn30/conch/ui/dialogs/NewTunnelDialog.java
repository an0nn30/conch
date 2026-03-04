package com.github.an0nn30.conch.ui.dialogs;

import com.github.an0nn30.conch.config.ConfigManager;
import com.github.an0nn30.conch.model.SavedTunnel;
import com.github.an0nn30.conch.model.ServerEntry;
import com.github.an0nn30.conch.model.ServerFolder;
import com.github.an0nn30.conch.ssh.TunnelManager;

import javax.swing.*;
import javax.swing.border.EmptyBorder;
import java.awt.*;
import java.util.ArrayList;
import java.util.List;

public class NewTunnelDialog extends JDialog {

    private final ConfigManager configManager;
    private final TunnelManager tunnelManager;
    private       Runnable      onConnected;

    private JComboBox<ServerEntry> serverCombo;
    private JTextField             localPortField;
    private JTextField             remoteHostField;
    private JTextField             remotePortField;
    private JTextField             labelField;
    private JLabel                 statusLabel;
    private JButton                connectBtn;

    public NewTunnelDialog(Window owner, ConfigManager configManager, TunnelManager tunnelManager) {
        super(owner, "New SSH Tunnel", ModalityType.APPLICATION_MODAL);
        this.configManager = configManager;
        this.tunnelManager = tunnelManager;
        setSize(460, 340);
        setResizable(false);
        setLocationRelativeTo(owner);
        buildUI();
    }

    /** Called (on the EDT) after a tunnel is successfully established. */
    public void setOnConnected(Runnable r) { this.onConnected = r; }

    // -----------------------------------------------------------------------

    private void buildUI() {
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(new EmptyBorder(20, 24, 8, 24));

        GridBagConstraints lc = new GridBagConstraints();
        lc.anchor = GridBagConstraints.WEST;
        lc.insets = new Insets(5, 0, 5, 12);
        lc.gridx  = 0;

        GridBagConstraints fc = new GridBagConstraints();
        fc.fill    = GridBagConstraints.HORIZONTAL;
        fc.weightx = 1.0;
        fc.insets  = new Insets(5, 0, 5, 0);
        fc.gridx   = 1;

        int row = 0;

        // SSH Server
        lc.gridy = row; fc.gridy = row++;
        form.add(new JLabel("SSH Server:"), lc);
        serverCombo = new JComboBox<>(allServers().toArray(new ServerEntry[0]));
        serverCombo.setRenderer(serverRenderer());
        form.add(serverCombo, fc);

        // Local Port
        lc.gridy = row; fc.gridy = row++;
        form.add(new JLabel("Local Port:"), lc);
        localPortField = new JTextField(8);
        JPanel localRow = new JPanel(new FlowLayout(FlowLayout.LEFT, 0, 0));
        localRow.add(localPortField);
        form.add(localRow, fc);

        // Remote Host
        lc.gridy = row; fc.gridy = row++;
        form.add(new JLabel("Remote Host:"), lc);
        remoteHostField = new JTextField("localhost");
        form.add(remoteHostField, fc);

        // Remote Port
        lc.gridy = row; fc.gridy = row++;
        form.add(new JLabel("Remote Port:"), lc);
        remotePortField = new JTextField(8);
        JPanel remoteRow = new JPanel(new FlowLayout(FlowLayout.LEFT, 0, 0));
        remoteRow.add(remotePortField);
        form.add(remoteRow, fc);

        // Label (optional)
        lc.gridy = row; fc.gridy = row++;
        form.add(new JLabel("Label (opt.):"), lc);
        labelField = new JTextField();
        form.add(labelField, fc);

        // Status line
        statusLabel = new JLabel(" ");
        statusLabel.setForeground(UIManager.getColor("Label.errorForeground") != null
                ? UIManager.getColor("Label.errorForeground") : Color.RED);
        statusLabel.setBorder(new EmptyBorder(0, 24, 0, 24));

        // Buttons
        connectBtn = new JButton("Save & Connect");
        JButton cancelBtn = new JButton("Cancel");
        connectBtn.addActionListener(e -> connect());
        cancelBtn.addActionListener(e -> dispose());
        getRootPane().setDefaultButton(connectBtn);

        JPanel buttons = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 8));
        buttons.add(cancelBtn);
        buttons.add(connectBtn);

        setLayout(new BorderLayout());
        add(form,        BorderLayout.CENTER);

        // Wrap status + buttons together below the form
        JPanel south = new JPanel(new BorderLayout());
        south.add(statusLabel, BorderLayout.NORTH);
        south.add(buttons,     BorderLayout.SOUTH);
        add(south, BorderLayout.SOUTH);

        // Warn if no servers are configured
        if (serverCombo.getItemCount() == 0) {
            connectBtn.setEnabled(false);
            statusLabel.setText("No SSH servers configured. Add one in the sidebar first.");
        }
    }

    // -----------------------------------------------------------------------

    private void connect() {
        // -- Validate ---------------------------------------------------------
        int localPort = parsePort(localPortField.getText(), "Local Port");
        if (localPort < 0) return;

        String remoteHost = remoteHostField.getText().strip();
        if (remoteHost.isEmpty()) { statusLabel.setText("Remote host is required."); return; }

        int remotePort = parsePort(remotePortField.getText(), "Remote Port");
        if (remotePort < 0) return;

        ServerEntry server = (ServerEntry) serverCombo.getSelectedItem();
        if (server == null) { statusLabel.setText("Select an SSH server."); return; }

        // -- Create SavedTunnel and persist -----------------------------------
        SavedTunnel saved = new SavedTunnel(
                server.getHost(), server.getPort(), server.getUsername(),
                localPort, remoteHost, remotePort, labelField.getText());

        configManager.getConfig().getTunnels().add(saved);
        configManager.saveConfig();

        // -- Activate in background -------------------------------------------
        connectBtn.setEnabled(false);
        connectBtn.setText("Connecting\u2026");
        statusLabel.setText(" ");

        SwingWorker<Void, Void> worker = new SwingWorker<>() {
            @Override
            protected Void doInBackground() throws Exception {
                tunnelManager.activate(saved, server);
                return null;
            }

            @Override
            protected void done() {
                connectBtn.setEnabled(true);
                connectBtn.setText("Save & Connect");
                try {
                    get();
                    if (onConnected != null) onConnected.run();
                    dispose();
                } catch (Exception e) {
                    Throwable cause = e.getCause() != null ? e.getCause() : e;
                    statusLabel.setText(cause.getMessage());
                }
            }
        };
        worker.execute();
    }

    // -----------------------------------------------------------------------

    private List<ServerEntry> allServers() {
        List<ServerEntry> result = new ArrayList<>();
        for (ServerFolder f : configManager.getConfig().getFolders()) {
            result.addAll(f.getServers());
        }
        return result;
    }

    private static ListCellRenderer<? super ServerEntry> serverRenderer() {
        return new DefaultListCellRenderer() {
            @Override
            public Component getListCellRendererComponent(JList<?> list, Object value,
                    int index, boolean isSelected, boolean cellHasFocus) {
                super.getListCellRendererComponent(list, value, index, isSelected, cellHasFocus);
                if (value instanceof ServerEntry s) {
                    setText(s.getName() + " \u2014 " + s.getUsername() + "@" + s.getHost());
                }
                return this;
            }
        };
    }

    private int parsePort(String text, String fieldName) {
        try {
            int port = Integer.parseInt(text.strip());
            if (port < 1 || port > 65535) throw new NumberFormatException();
            return port;
        } catch (NumberFormatException e) {
            statusLabel.setText(fieldName + " must be a number between 1 and 65535.");
            return -1;
        }
    }
}
