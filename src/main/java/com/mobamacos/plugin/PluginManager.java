package com.mobamacos.plugin;

import java.io.IOException;
import java.nio.file.*;
import java.util.*;
import java.util.stream.Stream;

/**
 * Discovers {@code .groovy} plugin scripts under {@code ~/.mobamacos/plugins/}
 * and parses their header metadata.
 */
public class PluginManager {

    private final Path pluginDir;

    public PluginManager(Path configDir) {
        this.pluginDir = configDir.resolve("plugins");
    }

    /** Ensures the plugins directory exists and returns all discovered plugins. */
    public List<PluginScript> scan() {
        try {
            Files.createDirectories(pluginDir);
        } catch (IOException ignored) {}

        if (!Files.isDirectory(pluginDir)) return List.of();

        try (Stream<Path> files = Files.list(pluginDir)) {
            return files
                    .filter(p -> p.toString().endsWith(".groovy"))
                    .sorted()
                    .map(this::parse)
                    .filter(Objects::nonNull)
                    .toList();
        } catch (IOException e) {
            System.err.println("[PluginManager] scan error: " + e.getMessage());
            return List.of();
        }
    }

    public Path getPluginDir() { return pluginDir; }

    // -----------------------------------------------------------------------

    private PluginScript parse(Path path) {
        try {
            List<String> lines = Files.readAllLines(path);
            String name        = null;
            String description = "";
            String version     = "1.0";

            for (String line : lines) {
                String stripped = line.strip();
                if (!stripped.startsWith("//")) continue;
                String body = stripped.substring(2).strip();

                if (body.startsWith("plugin-name:"))
                    name = body.substring("plugin-name:".length()).strip();
                else if (body.startsWith("plugin-description:"))
                    description = body.substring("plugin-description:".length()).strip();
                else if (body.startsWith("plugin-version:"))
                    version = body.substring("plugin-version:".length()).strip();
            }

            if (name == null || name.isBlank()) {
                // Fall back to file name without extension
                String fname = path.getFileName().toString();
                name = fname.substring(0, fname.lastIndexOf('.'));
            }

            return new PluginScript(name, description, version, path);
        } catch (IOException e) {
            System.err.println("[PluginManager] error reading " + path + ": " + e.getMessage());
            return null;
        }
    }
}
