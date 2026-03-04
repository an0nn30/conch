package com.mobamacos.plugin;

import groovy.lang.Binding;
import groovy.lang.GroovyClassLoader;
import groovy.lang.GroovyShell;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.function.Consumer;

/**
 * Executes Groovy plugin scripts via {@link GroovyShell}.
 *
 * <p>Each script runs on a dedicated daemon thread so it cannot block the EDT.
 * Grape's root directory is set to {@code ~/.mobamacos/grapes/} so that
 * {@code @Grab} dependencies are cached between runs.
 *
 * <p>The thread's context classloader is set to a {@link GroovyClassLoader} whose
 * parent is the application classloader.  Grape resolves {@code GrapeIvy} via the
 * context classloader, so without this daemon threads (which inherit a minimal
 * bootstrap classloader) cannot find Ivy and {@code @Grab} silently fails.
 */
public class PluginRunner {

    private final PluginContext context;
    private final Path          grapeRoot;
    /** Captured at construction time — the application's classloader. */
    private final ClassLoader   appClassLoader = getClass().getClassLoader();

    public PluginRunner(PluginContext context, Path configDir) {
        this.context   = context;
        this.grapeRoot = configDir.resolve("grapes");
    }

    /**
     * Runs {@code plugin} on a daemon thread.
     *
     * @param plugin         the script to execute
     * @param outputCallback receives output lines (including errors)
     * @return the thread executing the script (already started)
     */
    public Thread run(PluginScript plugin, Consumer<String> outputCallback) {
        Thread t = new Thread(() -> {
            try {
                Files.createDirectories(grapeRoot);
                System.setProperty("grape.root", grapeRoot.toString());

                // Give this thread a GroovyClassLoader so Grape can both find GrapeIvy
                // (via the app classloader parent) and add downloaded JARs at runtime.
                GroovyClassLoader gcl = new GroovyClassLoader(appClassLoader);
                Thread.currentThread().setContextClassLoader(gcl);

                String  source  = Files.readString(plugin.path());
                Binding binding = context.createBinding(outputCallback);
                new GroovyShell(gcl, binding).evaluate(source);

            } catch (Exception e) {
                outputCallback.accept("[ERROR] " + e.getMessage());
                for (StackTraceElement el : e.getStackTrace()) {
                    outputCallback.accept("  at " + el);
                    if (el.getClassName().startsWith("Script")) break;
                }
            }
        }, "plugin-" + plugin.name());
        t.setDaemon(true);
        t.start();
        return t;
    }
}
