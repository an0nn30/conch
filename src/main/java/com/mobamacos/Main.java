package com.mobamacos;

import com.formdev.flatlaf.FlatIntelliJLaf;
import com.mobamacos.config.ConfigManager;
import com.mobamacos.theme.ThemeManager;
import com.mobamacos.ui.MainWindow;

import javax.swing.*;
import java.awt.*;

public class Main {
    public static void main(String[] args) {
        // macOS-specific tweaks
        System.setProperty("com.apple.mrj.application.apple.menu.about.name", "MobaMacOS");
        System.setProperty("apple.awt.application.name", "MobaMacOS Terminal");

        SwingUtilities.invokeLater(() -> {
            try {
                ConfigManager configManager = new ConfigManager();
                ThemeManager  themeManager  = new ThemeManager(configManager);
                themeManager.applyTheme();

                // Tab appearance tweaks (after L&F is applied)
                applyTabStyle();

                MainWindow window = new MainWindow(configManager, themeManager);
                window.setVisible(true);
                window.handleStartup();   // must be after setVisible

            } catch (Exception e) {
                e.printStackTrace();
                JOptionPane.showMessageDialog(null,
                        "Fatal startup error:\n" + e.getMessage(),
                        "MobaMacOS", JOptionPane.ERROR_MESSAGE);
            }
        });
    }

    private static void applyTabStyle() {
        // Make tabs look closer to MobaXterm — flat, slightly taller, no painted border on content
        UIManager.put("TabbedPane.tabHeight", 30);
        UIManager.put("TabbedPane.tabInsets", new Insets(2, 10, 2, 4));
        UIManager.put("TabbedPane.selectedBackground",
                UIManager.getColor("Panel.background"));
        UIManager.put("TabbedPane.showTabSeparators", true);
        UIManager.put("TabbedPane.tabSeparatorsFullHeight", false);
    }
}
