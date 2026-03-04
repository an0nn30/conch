package com.mobamacos.plugin.api;

import javax.swing.*;
import java.awt.*;

/**
 * A live-streaming output window returned by {@code ui.outputPanel(title)}.
 * Call {@link #appendLine(String)} from any thread; updates are dispatched to the EDT.
 */
public class OutputPanel {

    private final JDialog  dialog;
    private final JTextArea area;

    public OutputPanel(String title) {
        dialog = new JDialog((Frame) null, title, false);  // modeless
        dialog.setDefaultCloseOperation(JDialog.DISPOSE_ON_CLOSE);
        dialog.setSize(700, 450);
        dialog.setLocationRelativeTo(null);

        area = new JTextArea();
        area.setEditable(false);
        area.setFont(new Font(Font.MONOSPACED, Font.PLAIN, 12));
        area.setLineWrap(true);
        area.setWrapStyleWord(true);

        JScrollPane scroll = new JScrollPane(area);
        dialog.add(scroll, BorderLayout.CENTER);

        JButton close = new JButton("Close");
        close.addActionListener(e -> dialog.dispose());
        JPanel footer = new JPanel(new FlowLayout(FlowLayout.RIGHT));
        footer.add(close);
        dialog.add(footer, BorderLayout.SOUTH);

        dialog.setVisible(true);
    }

    /** Append a line of text. Safe to call from any thread. */
    public void appendLine(String line) {
        SwingUtilities.invokeLater(() -> {
            area.append(line);
            if (!line.endsWith("\n")) area.append("\n");
            area.setCaretPosition(area.getDocument().getLength());
        });
    }

    public void clear() {
        SwingUtilities.invokeLater(() -> area.setText(""));
    }

    public void close() {
        SwingUtilities.invokeLater(dialog::dispose);
    }
}
