package com.github.an0nn30.conch.config;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;
import com.github.an0nn30.conch.model.AppConfig;
import com.github.an0nn30.conch.model.ServerFolder;
import com.github.an0nn30.conch.sshconfig.SshConfigParser;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

public class ConfigManager {
    private static final Path CONFIG_DIR  = Paths.get(System.getProperty("user.home"), ".config", "conch");
    private static final Path CONFIG_FILE = CONFIG_DIR.resolve("config.json");

    private AppConfig config;
    private final ObjectMapper mapper;

    public ConfigManager() {
        mapper = new ObjectMapper().enable(SerializationFeature.INDENT_OUTPUT);
        loadConfig();
        refreshSshConfig();  // always re-read from disk — never trust the persisted copy
    }

    private void loadConfig() {
        try {
            File f = CONFIG_FILE.toFile();
            if (f.exists()) {
                config = mapper.readValue(f, AppConfig.class);
            } else {
                config = createDefaultConfig();
                saveConfig();
            }
        } catch (IOException e) {
            System.err.println("Could not load config, using defaults: " + e.getMessage());
            config = createDefaultConfig();
        }
    }

    /**
     * Always removes any previously-persisted "SSH Config" folder and re-parses
     * ~/.ssh/config from disk.  This guarantees ProxyCommand, HostName, and
     * IdentityFile are always current — stale persisted copies caused connections
     * to attempt direct TCP when ProxyCommand was missing from the JSON.
     */
    private void refreshSshConfig() {
        config.getFolders().removeIf(f -> "SSH Config".equals(f.getName()));
        ServerFolder fresh = SshConfigParser.parse();
        if (!fresh.getServers().isEmpty()) {
            config.getFolders().add(0, fresh);
        }
    }

    private AppConfig createDefaultConfig() {
        AppConfig cfg = new AppConfig();
        cfg.getFolders().add(new ServerFolder("My Servers"));
        return cfg;
    }

    /**
     * Persists user-created folders only.  SSH Config entries are derived from
     * ~/.ssh/config on every startup, so we never write them to disk — avoids
     * stale data (missing fields from older schema, outdated ProxyCommand, etc).
     */
    public void saveConfig() {
        try {
            Files.createDirectories(CONFIG_DIR);
            AppConfig toSave = configWithoutSshConfig();
            mapper.writeValue(CONFIG_FILE.toFile(), toSave);
        } catch (IOException e) {
            System.err.println("Could not save config: " + e.getMessage());
        }
    }

    private AppConfig configWithoutSshConfig() {
        AppConfig copy = new AppConfig();
        copy.setTheme(config.getTheme());
        copy.setFontSize(config.getFontSize());
        copy.setFontName(config.getFontName());
        copy.setTerminalBackground(config.getTerminalBackground());
        copy.setTerminalForeground(config.getTerminalForeground());
        copy.setTunnels(config.getTunnels());
        for (ServerFolder f : config.getFolders()) {
            if (!"SSH Config".equals(f.getName())) {
                copy.getFolders().add(f);
            }
        }
        return copy;
    }

    public AppConfig getConfig() { return config; }
}
