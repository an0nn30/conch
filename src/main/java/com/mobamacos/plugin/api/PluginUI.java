package com.mobamacos.plugin.api;

import groovy.lang.Closure;

import javax.swing.*;
import javax.swing.border.EmptyBorder;
import java.awt.*;
import java.util.List;
import java.util.*;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.SynchronousQueue;
import java.util.function.Consumer;

/**
 * The {@code ui} binding object available in every plugin script.
 *
 * All methods are safe to call from the plugin's background thread; they
 * dispatch to the EDT internally and block until the user responds.
 */
public class PluginUI {

    private final Consumer<String> outputConsumer; // writes to ToolsPanel output area

    public PluginUI(Consumer<String> outputConsumer) {
        this.outputConsumer = outputConsumer;
    }

    // -----------------------------------------------------------------------
    // Forms
    // -----------------------------------------------------------------------

    /**
     * Shows a modal input form built from the DSL closure.
     * Returns a {@code Map<String,Object>} of field-name → value, or {@code null}
     * if the user cancelled.
     *
     * <pre>{@code
     * def v = ui.form("Deploy") {
     *     textField "host",  label: "Hostname"
     *     comboBox  "env",   label: "Env", items: ["prod","staging"]
     *     checkBox  "dry",   label: "Dry run", value: false
     * }
     * if (!v) return  // cancelled
     * }</pre>
     */
    public Map<String, Object> form(String title, Closure<?> definition) throws Exception {
        FormBuilder builder = new FormBuilder();
        definition.setDelegate(builder);
        definition.setResolveStrategy(Closure.DELEGATE_FIRST);
        definition.call();

        SynchronousQueue<Optional<Map<String, Object>>> q = new SynchronousQueue<>();
        SwingUtilities.invokeLater(() -> builder.buildAndShow(title, q));
        return q.take().orElse(null);
    }

    // -----------------------------------------------------------------------
    // Simple prompts
    // -----------------------------------------------------------------------

    /** Single-field text prompt. Returns {@code null} if cancelled. */
    public String prompt(String message) throws Exception {
        String[] result = {null};
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            result[0] = JOptionPane.showInputDialog(null, message);
            latch.countDown();
        });
        latch.await();
        return result[0];
    }

    /** Confirmation dialog. Returns {@code true} if the user clicked Yes. */
    public boolean confirm(String message) throws Exception {
        int[] result = {JOptionPane.NO_OPTION};
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            result[0] = JOptionPane.showConfirmDialog(null, message,
                    "Confirm", JOptionPane.YES_NO_OPTION);
            latch.countDown();
        });
        latch.await();
        return result[0] == JOptionPane.YES_OPTION;
    }

    // -----------------------------------------------------------------------
    // Alerts
    // -----------------------------------------------------------------------

    /** Information dialog. Blocks until dismissed. */
    public void alert(String title, String message) throws Exception {
        showDialog(title, message, JOptionPane.INFORMATION_MESSAGE);
    }

    /** Error dialog. Blocks until dismissed. */
    public void error(String title, String message) throws Exception {
        showDialog(title, message, JOptionPane.ERROR_MESSAGE);
    }

    private void showDialog(String title, String message, int type) throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            JOptionPane.showMessageDialog(null, message, title, type);
            latch.countDown();
        });
        latch.await();
    }

    // -----------------------------------------------------------------------
    // Results display
    // -----------------------------------------------------------------------

    /**
     * Shows a scrollable read-only text result dialog. Blocks until dismissed.
     *
     * <pre>{@code
     * ui.show("Pod List", podOutput)
     * }</pre>
     */
    public void show(String title, String text) throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            JTextArea area = new JTextArea(text);
            area.setEditable(false);
            area.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 12));
            area.setLineWrap(true);
            area.setWrapStyleWord(true);

            JScrollPane scroll = new JScrollPane(area);
            scroll.setPreferredSize(new Dimension(640, 400));

            JButton close = new JButton("Close");
            JButton copy  = new JButton("Copy");
            copy.addActionListener(e -> {
                var sel = new java.awt.datatransfer.StringSelection(text);
                Toolkit.getDefaultToolkit().getSystemClipboard().setContents(sel, null);
            });

            JPanel footer = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 0));
            footer.add(copy);
            footer.add(close);

            JDialog d = new JDialog((Frame) null, title, true);
            d.setLayout(new BorderLayout(0, 8));
            d.getRootPane().setBorder(new EmptyBorder(12, 12, 12, 12));
            d.add(scroll,  BorderLayout.CENTER);
            d.add(footer,  BorderLayout.SOUTH);
            d.pack();
            d.setLocationRelativeTo(null);
            close.addActionListener(e -> d.dispose());
            d.addWindowListener(new java.awt.event.WindowAdapter() {
                @Override public void windowClosed(java.awt.event.WindowEvent e) { latch.countDown(); }
            });
            d.setVisible(true);
        });
        latch.await();
    }

    /**
     * Shows a results dialog with custom action buttons built from a DSL closure.
     *
     * <pre>{@code
     * ui.results("Fetched Secret") {
     *     text  decodedText
     *     button("Copy All") { app.clipboard(decodedText) }
     * }
     * }</pre>
     */
    public void results(String title, Closure<?> definition) throws Exception {
        ResultsBuilder builder = new ResultsBuilder();
        definition.setDelegate(builder);
        definition.setResolveStrategy(Closure.DELEGATE_FIRST);
        definition.call();

        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> builder.buildAndShow(title, latch));
        latch.await();
    }

    /** Convenience overload: text + no extra buttons. */
    public void results(String title, String text) throws Exception {
        show(title, text);
    }

    // -----------------------------------------------------------------------
    // Tables
    // -----------------------------------------------------------------------

    /**
     * Displays data in a sortable table dialog.
     *
     * @param columns column header names
     * @param rows    each element is a List matching the column order
     */
    public void table(String title, List<String> columns, List<List<Object>> rows) throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            String[] cols = columns.toArray(new String[0]);
            Object[][] data = rows.stream()
                    .map(r -> r.toArray(new Object[0]))
                    .toArray(Object[][]::new);

            JTable table = new JTable(data, cols);
            table.setAutoCreateRowSorter(true);
            table.setFillsViewportHeight(true);

            JScrollPane scroll = new JScrollPane(table);
            scroll.setPreferredSize(new Dimension(700, 360));

            JButton close = new JButton("Close");
            JPanel footer = new JPanel(new FlowLayout(FlowLayout.RIGHT));
            footer.add(close);

            JDialog d = new JDialog((Frame) null, title, true);
            d.setLayout(new BorderLayout());
            d.add(scroll,  BorderLayout.CENTER);
            d.add(footer,  BorderLayout.SOUTH);
            d.pack();
            d.setLocationRelativeTo(null);
            close.addActionListener(e -> d.dispose());
            d.addWindowListener(new java.awt.event.WindowAdapter() {
                @Override public void windowClosed(java.awt.event.WindowEvent e) { latch.countDown(); }
            });
            d.setVisible(true);
        });
        latch.await();
    }

    // -----------------------------------------------------------------------
    // Progress
    // -----------------------------------------------------------------------

    /**
     * Shows a modeless progress dialog while {@code work} runs, then dismisses it.
     *
     * <pre>{@code
     * ui.withProgress("Querying nodes...") {
     *     doWork()
     * }
     * }</pre>
     */
    public void withProgress(String message, Closure<?> work) throws Exception {
        CountDownLatch shown = new CountDownLatch(1);
        JDialog[] ref = {null};

        SwingUtilities.invokeLater(() -> {
            JDialog d = new JDialog((Frame) null, message, false);
            d.setDefaultCloseOperation(JDialog.DO_NOTHING_ON_CLOSE);
            d.setSize(340, 90);
            d.setLocationRelativeTo(null);
            d.setLayout(new BorderLayout(12, 12));

            JProgressBar bar = new JProgressBar();
            bar.setIndeterminate(true);
            JLabel lbl = new JLabel(message, SwingConstants.CENTER);

            d.add(lbl, BorderLayout.NORTH);
            d.add(bar, BorderLayout.CENTER);
            d.getRootPane().setBorder(new EmptyBorder(12, 20, 12, 20));
            d.setVisible(true);
            ref[0] = d;
            shown.countDown();
        });

        shown.await();
        try {
            work.call();
        } finally {
            SwingUtilities.invokeLater(() -> { if (ref[0] != null) ref[0].dispose(); });
        }
    }

    // -----------------------------------------------------------------------
    // Live output panel
    // -----------------------------------------------------------------------

    /**
     * Opens a modeless live-streaming output window.  Call {@link OutputPanel#appendLine}
     * from any thread to push new lines.
     */
    public OutputPanel outputPanel(String title) throws Exception {
        OutputPanel[] ref = {null};
        CountDownLatch latch = new CountDownLatch(1);
        SwingUtilities.invokeLater(() -> {
            ref[0] = new OutputPanel(title);
            latch.countDown();
        });
        latch.await();
        return ref[0];
    }

    // -----------------------------------------------------------------------
    // Convenience: write to the Tools-panel output area
    // -----------------------------------------------------------------------

    /** Append a line to the Tools-panel output area (non-blocking). */
    public void append(String line) {
        outputConsumer.accept(line);
    }

    // -----------------------------------------------------------------------
    // Inner: results DSL builder
    // -----------------------------------------------------------------------

    public static class ResultsBuilder {
        private String text = "";
        private final List<ButtonDef> buttons = new ArrayList<>();

        record ButtonDef(String label, Closure<?> action) {}

        public void text(String t) { this.text = t; }
        public void button(String label, Closure<?> action) { buttons.add(new ButtonDef(label, action)); }

        void buildAndShow(String title, CountDownLatch latch) {
            JTextArea area = new JTextArea(text);
            area.setEditable(false);
            area.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 12));
            area.setLineWrap(true);
            area.setWrapStyleWord(true);

            JScrollPane scroll = new JScrollPane(area);
            scroll.setPreferredSize(new Dimension(640, 380));

            JPanel footer = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 0));

            JDialog d = new JDialog((Frame) null, title, true);

            for (ButtonDef bd : buttons) {
                JButton btn = new JButton(bd.label());
                btn.addActionListener(e -> {
                    // run button actions on a daemon thread so they can call
                    // session.exec() or other blocking APIs safely
                    Thread t = new Thread(() -> {
                        try { bd.action().call(text); } catch (Exception ex) {
                            SwingUtilities.invokeLater(() ->
                                JOptionPane.showMessageDialog(d, ex.getMessage(),
                                        "Button Error", JOptionPane.ERROR_MESSAGE));
                        }
                    }, "plugin-btn");
                    t.setDaemon(true);
                    t.start();
                });
                footer.add(btn);
            }

            JButton close = new JButton("Close");
            close.addActionListener(e -> d.dispose());
            footer.add(close);

            d.setLayout(new BorderLayout(0, 8));
            d.getRootPane().setBorder(new EmptyBorder(12, 12, 12, 12));
            d.add(scroll, BorderLayout.CENTER);
            d.add(footer, BorderLayout.SOUTH);
            d.pack();
            d.setLocationRelativeTo(null);
            d.addWindowListener(new java.awt.event.WindowAdapter() {
                @Override public void windowClosed(java.awt.event.WindowEvent e) { latch.countDown(); }
            });
            d.setVisible(true);
        }
    }
}
