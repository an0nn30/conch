package com.mobamacos.plugin;

import java.nio.file.Path;

/**
 * Metadata parsed from a {@code .groovy} plugin file's header comments.
 *
 * <pre>{@code
 * // plugin-name: My Tool
 * // plugin-description: Does something useful
 * // plugin-version: 1.0
 * }</pre>
 */
public record PluginScript(
        String name,
        String description,
        String version,
        Path   path) {

    @Override public String toString() { return name; }
}
