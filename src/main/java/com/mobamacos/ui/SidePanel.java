package com.mobamacos.ui;

import com.mobamacos.ui.files.FileTransferPanel;

import javax.swing.*;
import java.awt.*;
import java.awt.geom.AffineTransform;

/**
 * Left-side panel: vertical tab bar (Files / Tools / Macros) with content area.
 * Sessions have moved to the right-side {@link SessionsPanel}.
 */
public class SidePanel extends JPanel {

    private final FileTransferPanel filePanel;
    private final ToolsPanel        toolsPanel;

    public SidePanel(FileTransferPanel filePanel, ToolsPanel toolsPanel) {
        this.filePanel  = filePanel;
        this.toolsPanel = toolsPanel;
        initUI();
    }

    // stub kept so MenuBarBuilder compiles without changes
    public void refresh() {}

    // -----------------------------------------------------------------------
    // Build UI
    // -----------------------------------------------------------------------

    private void initUI() {
        setLayout(new BorderLayout());
        setPreferredSize(new Dimension(260, 0));

        VerticalTabBar tabBar = new VerticalTabBar();

        CardLayout cards = new CardLayout();
        JPanel content = new JPanel(cards);
        content.add(filePanel,                    "Files");
        content.add(toolsPanel,                   "Tools");
        content.add(buildPlaceholderPanel("Macros"), "Macros");

        tabBar.addTab("Files",  AppIcons.TAB_FILES,  () -> cards.show(content, "Files"));
        tabBar.addTab("Tools",  AppIcons.TAB_TOOLS,  () -> cards.show(content, "Tools"));
        tabBar.addTab("Macros", AppIcons.TAB_MACROS, () -> cards.show(content, "Macros"));
        tabBar.selectFirst();

        add(tabBar,  BorderLayout.WEST);
        add(content, BorderLayout.CENTER);
    }

    private JPanel buildPlaceholderPanel(String name) {
        JPanel p = new JPanel(new GridBagLayout());
        JLabel lbl = new JLabel(name + " panel \u2014 coming soon");
        lbl.setForeground(Color.GRAY);
        p.add(lbl);
        return p;
    }

    // -----------------------------------------------------------------------
    // Vertical tab bar
    // -----------------------------------------------------------------------

    private static class VerticalTabBar extends JPanel {

        private final ButtonGroup group = new ButtonGroup();

        VerticalTabBar() {
            setLayout(new BoxLayout(this, BoxLayout.Y_AXIS));
            setBorder(BorderFactory.createMatteBorder(0, 0, 0, 1,
                    UIManager.getColor("Separator.foreground") != null
                            ? UIManager.getColor("Separator.foreground") : Color.LIGHT_GRAY));
        }

        void addTab(String label, String iconName, Runnable onSelect) {
            VTabButton btn = new VTabButton(label, AppIcons.get(iconName));
            group.add(btn);
            btn.addActionListener(e -> onSelect.run());
            add(btn);
        }

        void selectFirst() {
            Component first = getComponent(0);
            if (first instanceof VTabButton b) b.doClick();
        }
    }

    /**
     * Vertical toggle button: icon + label as a centred unit, rotated 90° CCW
     * so the label reads bottom→top with the icon at the word's start.
     */
    private static class VTabButton extends JToggleButton {

        private static final int WIDTH   = 28;
        private static final int ICON_SZ = 16;
        private static final int GAP     = 5;

        private final ImageIcon tabIcon;

        VTabButton(String text, ImageIcon icon) {
            super(text);
            this.tabIcon = icon;
            setFont(getFont().deriveFont(Font.PLAIN, 11f));
            setFocusPainted(false);
        }

        @Override
        public Dimension getPreferredSize() {
            FontMetrics fm = getFontMetrics(getFont());
            int h = fm.stringWidth(getText()) + GAP + ICON_SZ + 24;
            return new Dimension(WIDTH, h);
        }

        @Override public Dimension getMinimumSize() { return getPreferredSize(); }
        @Override public Dimension getMaximumSize() {
            Dimension d = getPreferredSize();
            return new Dimension(d.width, Short.MAX_VALUE);
        }

        @Override
        protected void paintComponent(Graphics g) {
            Graphics2D g2 = (Graphics2D) g.create();
            g2.setRenderingHint(RenderingHints.KEY_TEXT_ANTIALIASING,
                    RenderingHints.VALUE_TEXT_ANTIALIAS_ON);
            g2.setRenderingHint(RenderingHints.KEY_RENDERING,
                    RenderingHints.VALUE_RENDER_QUALITY);

            Color bg = UIManager.getColor("Panel.background");
            if (bg == null) bg = getBackground();
            g2.setColor(isSelected() ? bg : new Color(
                    Math.max(0, bg.getRed()   - 18),
                    Math.max(0, bg.getGreen() - 18),
                    Math.max(0, bg.getBlue()  - 18)));
            g2.fillRect(0, 0, getWidth(), getHeight());

            if (isSelected()) {
                Color accent = UIManager.getColor("Component.accentColor");
                g2.setColor(accent != null ? accent : new Color(75, 110, 175));
                g2.fillRect(getWidth() - 3, 0, 3, getHeight());
            }

            g2.setFont(getFont());
            FontMetrics fm = g2.getFontMetrics();
            int tw  = fm.stringWidth(getText());
            int cx  = getWidth() / 2;

            int totalBlock = tw + GAP + ICON_SZ;
            int blockStart = (getHeight() - totalBlock) / 2;

            g2.setColor(getForeground());
            int textCentreY = blockStart + tw / 2;
            AffineTransform saved = g2.getTransform();
            g2.translate(cx, textCentreY);
            g2.rotate(-Math.PI / 2);
            g2.drawString(getText(), -tw / 2, fm.getAscent() / 2 - 1);
            g2.setTransform(saved);

            if (tabIcon != null) {
                int iconTop = blockStart + tw + GAP;
                g2.drawImage(tabIcon.getImage(), cx - ICON_SZ / 2, iconTop, ICON_SZ, ICON_SZ, null);
            }

            g2.dispose();
        }

        @Override public void paintBorder(Graphics g) {}
    }
}
