package com.mobamacos.plugin.api;

import javax.swing.*;
import javax.swing.border.EmptyBorder;
import java.awt.*;
import java.util.*;
import java.util.List;
import java.util.concurrent.SynchronousQueue;

/**
 * Delegate for the {@code ui.form(title) { ... }} DSL closure.
 *
 * Methods called inside the closure (textField, password, comboBox, …)
 * accumulate field definitions; {@link #buildAndShow} then constructs the
 * JDialog and waits for the user.
 *
 * Groovy named-parameter syntax means
 *   {@code textField "name", label: "Label", value: "default"}
 * arrives as  {@code textField("name", Map.of("label","Label","value","default"))}.
 */
public class FormBuilder {

    private enum FieldType { TEXT, PASSWORD, TEXTAREA, COMBO, CHECK, SPINNER, SEPARATOR, LABEL }

    private record FieldDef(FieldType type, String name, Map<String, Object> opts) {}

    private final List<FieldDef> fields = new ArrayList<>();

    // -----------------------------------------------------------------------
    // DSL methods (called from Groovy closure via delegate)
    // -----------------------------------------------------------------------

    public void textField(String name)                          { textField(name, Map.of()); }
    public void textField(String name, Map<String,Object> opts) { fields.add(new FieldDef(FieldType.TEXT,      name, opts)); }

    public void password(String name)                           { password(name, Map.of()); }
    public void password(String name, Map<String,Object> opts)  { fields.add(new FieldDef(FieldType.PASSWORD,  name, opts)); }

    public void textArea(String name)                           { textArea(name, Map.of()); }
    public void textArea(String name, Map<String,Object> opts)  { fields.add(new FieldDef(FieldType.TEXTAREA,  name, opts)); }

    public void comboBox(String name)                           { comboBox(name, Map.of()); }
    public void comboBox(String name, Map<String,Object> opts)  { fields.add(new FieldDef(FieldType.COMBO,     name, opts)); }

    public void checkBox(String name)                           { checkBox(name, Map.of()); }
    public void checkBox(String name, Map<String,Object> opts)  { fields.add(new FieldDef(FieldType.CHECK,     name, opts)); }

    public void spinner(String name)                            { spinner(name, Map.of()); }
    public void spinner(String name, Map<String,Object> opts)   { fields.add(new FieldDef(FieldType.SPINNER,   name, opts)); }

    public void separator() { fields.add(new FieldDef(FieldType.SEPARATOR, null, Map.of())); }

    public void label(String text) { fields.add(new FieldDef(FieldType.LABEL, null, Map.of("text", text))); }

    // -----------------------------------------------------------------------
    // Build + show (called by PluginUI from a background thread via invokeLater)
    // -----------------------------------------------------------------------

    /**
     * Must be called on the EDT.  Shows a modal dialog and puts the result map
     * (or {@code null} on cancel) into {@code resultQueue}.
     */
    void buildAndShow(String title, SynchronousQueue<Optional<Map<String, Object>>> resultQueue) {
        JDialog dialog = new JDialog((Frame) null, title, true);
        dialog.setDefaultCloseOperation(JDialog.DISPOSE_ON_CLOSE);

        // ---- Form panel ----
        JPanel form = new JPanel(new GridBagLayout());
        form.setBorder(new EmptyBorder(16, 20, 8, 20));
        GridBagConstraints lc = labelGbc(), fc = fieldGbc();
        int row = 0;

        Map<String, JComponent> inputs = new LinkedHashMap<>();

        for (FieldDef fd : fields) {
            switch (fd.type()) {
                case SEPARATOR -> {
                    GridBagConstraints wc = wideGbc(row++);
                    form.add(new JSeparator(), wc);
                }
                case LABEL -> {
                    GridBagConstraints wc = wideGbc(row++);
                    JLabel lbl = new JLabel(str(fd.opts(), "text", ""));
                    lbl.setForeground(Color.GRAY);
                    form.add(lbl, wc);
                }
                default -> {
                    lc.gridy = row; fc.gridy = row++;
                    String label = str(fd.opts(), "label", fd.name());
                    form.add(new JLabel(label + ":"), lc);
                    JComponent comp = buildInput(fd);
                    inputs.put(fd.name(), comp);
                    form.add(fd.type() == FieldType.TEXTAREA
                            ? new JScrollPane(comp) : comp, fc);
                }
            }
        }

        // ---- Buttons ----
        boolean[] confirmed = {false};
        JButton ok     = new JButton("OK");
        JButton cancel = new JButton("Cancel");
        ok.setDefaultCapable(true);
        ok.addActionListener(e -> { confirmed[0] = true; dialog.dispose(); });
        cancel.addActionListener(e -> dialog.dispose());

        JPanel buttons = new JPanel(new FlowLayout(FlowLayout.RIGHT, 8, 8));
        buttons.add(cancel);
        buttons.add(ok);

        dialog.setLayout(new BorderLayout());
        dialog.add(form,    BorderLayout.CENTER);
        dialog.add(buttons, BorderLayout.SOUTH);
        dialog.getRootPane().setDefaultButton(ok);

        dialog.pack();
        if (dialog.getWidth() < 440) dialog.setSize(440, dialog.getHeight());
        dialog.setLocationRelativeTo(null);

        dialog.addWindowListener(new java.awt.event.WindowAdapter() {
            @Override public void windowClosed(java.awt.event.WindowEvent e) {
                Optional<Map<String, Object>> result = confirmed[0]
                        ? Optional.of(collectValues(inputs))
                        : Optional.empty();
                try { resultQueue.put(result); } catch (InterruptedException ignored) {}
            }
        });

        dialog.setVisible(true);  // blocks EDT in nested event loop until closed
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    private JComponent buildInput(FieldDef fd) {
        Map<String, Object> o = fd.opts();
        return switch (fd.type()) {
            case TEXT -> {
                JTextField tf = new JTextField(str(o, "value", ""), 24);
                tf.putClientProperty("JTextField.placeholderText", str(o, "placeholder", ""));
                yield tf;
            }
            case PASSWORD -> new JPasswordField(str(o, "value", ""), 24);
            case TEXTAREA -> {
                JTextArea ta = new JTextArea(str(o, "value", ""),
                        num(o, "rows", 4), 24);
                ta.setLineWrap(true);
                ta.setWrapStyleWord(true);
                yield ta;
            }
            case COMBO -> {
                List<?> items = o.containsKey("items") ? (List<?>) o.get("items") : List.of();
                JComboBox<Object> cb = new JComboBox<>(items.toArray());
                Object def = o.get("value");
                if (def != null) cb.setSelectedItem(def);
                yield cb;
            }
            case CHECK -> {
                JCheckBox cb = new JCheckBox();
                cb.setSelected(Boolean.TRUE.equals(o.get("value")));
                yield cb;
            }
            case SPINNER -> {
                int val = num(o, "value", 0), min = num(o, "min", 0), max = num(o, "max", Integer.MAX_VALUE);
                JSpinner sp = new JSpinner(new SpinnerNumberModel(val, min, max, 1));
                sp.setPreferredSize(new Dimension(100, sp.getPreferredSize().height));
                yield sp;
            }
            default -> new JTextField(24);
        };
    }

    private Map<String, Object> collectValues(Map<String, JComponent> inputs) {
        Map<String, Object> result = new LinkedHashMap<>();
        for (var entry : inputs.entrySet()) {
            JComponent comp = entry.getValue();
            Object val;
            if      (comp instanceof JPasswordField pf) val = new String(pf.getPassword());
            else if (comp instanceof JTextField     tf) val = tf.getText();
            else if (comp instanceof JTextArea      ta) val = ta.getText();
            else if (comp instanceof JComboBox<?>   cb) val = cb.getSelectedItem();
            else if (comp instanceof JCheckBox      cb) val = cb.isSelected();
            else if (comp instanceof JSpinner       sp) val = sp.getValue();
            else                                        val = null;
            result.put(entry.getKey(), val);
        }
        return result;
    }

    private static String str(Map<String, Object> m, String k, String def) {
        Object v = m.get(k);
        return v != null ? v.toString() : def;
    }
    private static int num(Map<String, Object> m, String k, int def) {
        Object v = m.get(k);
        return v instanceof Number n ? n.intValue() : def;
    }

    private static GridBagConstraints labelGbc() {
        GridBagConstraints c = new GridBagConstraints();
        c.gridx = 0; c.anchor = GridBagConstraints.WEST;
        c.insets = new Insets(5, 0, 5, 12);
        return c;
    }
    private static GridBagConstraints fieldGbc() {
        GridBagConstraints c = new GridBagConstraints();
        c.gridx = 1; c.fill = GridBagConstraints.HORIZONTAL;
        c.weightx = 1.0; c.insets = new Insets(5, 0, 5, 0);
        return c;
    }
    private static GridBagConstraints wideGbc(int row) {
        GridBagConstraints c = new GridBagConstraints();
        c.gridx = 0; c.gridy = row; c.gridwidth = 2;
        c.fill = GridBagConstraints.HORIZONTAL;
        c.insets = new Insets(6, 0, 6, 0);
        return c;
    }
}
