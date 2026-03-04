package com.mobamacos.ui;

import com.mobamacos.plugin.PluginContext;
import com.mobamacos.plugin.PluginManager;
import com.mobamacos.plugin.PluginRunner;
import com.mobamacos.plugin.PluginScript;

import javax.swing.*;
import java.awt.*;
import java.nio.file.Path;
import java.util.List;

/**
 * The "Tools" side-panel tab: lists Groovy plugins discovered in
 * {@code ~/.mobamacos/plugins/}, lets the user run them, and streams
 * their output into a console area.
 */
public class ToolsPanel extends JPanel {

    private final PluginManager pluginManager;
    private final PluginRunner  pluginRunner;

    private final DefaultListModel<PluginScript> listModel  = new DefaultListModel<>();
    private final JList<PluginScript>            pluginList = new JList<>(listModel);
    private final JTextArea                      outputArea = new JTextArea();
    private final JButton                        runBtn     = new JButton("Run");

    public ToolsPanel(PluginContext context, PluginManager pluginManager, Path configDir) {
        super(new BorderLayout());
        this.pluginManager = pluginManager;
        this.pluginRunner  = new PluginRunner(context, configDir);
        initUI();
        refresh();
    }

    /** Re-scans the plugins directory and refreshes the list. */
    public void refresh() {
        List<PluginScript> scripts = pluginManager.scan();
        listModel.clear();
        scripts.forEach(listModel::addElement);
        runBtn.setEnabled(false);
        pluginList.clearSelection();
    }

    // -----------------------------------------------------------------------

    private void initUI() {
        setBorder(BorderFactory.createEmptyBorder(4, 4, 4, 4));

        // ---- Plugin list ----
        pluginList.setSelectionMode(ListSelectionModel.SINGLE_SELECTION);
        pluginList.setCellRenderer(new PluginCellRenderer());
        pluginList.addListSelectionListener(e ->
                runBtn.setEnabled(pluginList.getSelectedValue() != null));

        JScrollPane listScroll = new JScrollPane(pluginList);
        listScroll.setPreferredSize(new Dimension(0, 130));

        // ---- Buttons ----
        JButton refreshBtn = new JButton("Refresh");
        runBtn.setEnabled(false);
        runBtn.addActionListener(e -> runSelected());
        refreshBtn.addActionListener(e -> refresh());

        JPanel btnRow = new JPanel(new FlowLayout(FlowLayout.LEFT, 4, 0));
        btnRow.add(runBtn);
        btnRow.add(refreshBtn);

        JPanel topSection = new JPanel(new BorderLayout(0, 2));
        topSection.add(new JLabel("Plugins"), BorderLayout.NORTH);
        topSection.add(listScroll, BorderLayout.CENTER);
        topSection.add(btnRow,     BorderLayout.SOUTH);

        // ---- Output area ----
        outputArea.setEditable(false);
        outputArea.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 11));
        JScrollPane outScroll = new JScrollPane(outputArea);

        JButton clearBtn = new JButton("Clear");
        clearBtn.addActionListener(e -> outputArea.setText(""));

        JPanel outHeader = new JPanel(new BorderLayout());
        outHeader.add(new JLabel("Output"), BorderLayout.WEST);
        outHeader.add(clearBtn,             BorderLayout.EAST);

        JPanel bottomSection = new JPanel(new BorderLayout(0, 2));
        bottomSection.add(outHeader, BorderLayout.NORTH);
        bottomSection.add(outScroll, BorderLayout.CENTER);

        // ---- Split ----
        JSplitPane split = new JSplitPane(JSplitPane.VERTICAL_SPLIT, topSection, bottomSection);
        split.setResizeWeight(0.35);
        split.setDividerSize(5);
        split.setBorder(null);

        add(split, BorderLayout.CENTER);
    }

    private void runSelected() {
        PluginScript script = pluginList.getSelectedValue();
        if (script == null) return;

        outputArea.append("--- Running: " + script.name() + " ---\n");
        pluginRunner.run(script, line ->
            SwingUtilities.invokeLater(() -> {
                outputArea.append(line + "\n");
                outputArea.setCaretPosition(outputArea.getDocument().getLength());
            }));
    }

    // -----------------------------------------------------------------------
    // Cell renderer: bold name + muted description
    // -----------------------------------------------------------------------

    private static class PluginCellRenderer extends DefaultListCellRenderer {
        @Override
        public Component getListCellRendererComponent(JList<?> list, Object value,
                int index, boolean isSelected, boolean cellHasFocus) {
            super.getListCellRendererComponent(list, value, index, isSelected, cellHasFocus);
            if (value instanceof PluginScript ps) {
                String desc = ps.description().isBlank() ? "" : " — " + ps.description();
                setText("<html><b>" + ps.name() + "</b>"
                        + "<font color=gray>" + desc + "</font></html>");
                setToolTipText("v" + ps.version() + "  \u2014  " + ps.path());
            }
            return this;
        }
    }
}
