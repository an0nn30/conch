package com.mobamacos.plugin.api;

import net.schmizz.sshj.SSHClient;
import net.schmizz.sshj.connection.channel.direct.Session;

import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.*;
import java.util.function.Consumer;

/**
 * A handle to one open session (SSH or local) exposed to plugin scripts.
 *
 * <pre>{@code
 * def out = session.current.exec("df -h")
 * session.current.send("cd /var/log\n")
 * }</pre>
 */
public class SessionHandle {

    private final String    name;
    private final String    host;   // null for local
    private final String    user;
    private volatile String cwd;

    /** Non-null for SSH sessions; null for local. */
    private final SSHClient sshClient;

    /** The connector's write method — for send(). May be null for detached handles. */
    private final Consumer<String> terminalWriter;

    private static final int DEFAULT_TIMEOUT_SECONDS = 30;

    // SSH constructor
    public SessionHandle(String name, String host, String user, String cwd,
                         SSHClient sshClient, Consumer<String> terminalWriter) {
        this.name           = name;
        this.host           = host;
        this.user           = user;
        this.cwd            = cwd;
        this.sshClient      = sshClient;
        this.terminalWriter = terminalWriter;
    }

    // Local constructor
    public SessionHandle(String name, Consumer<String> terminalWriter) {
        this.name           = name;
        this.host           = null;
        this.user           = System.getProperty("user.name");
        this.cwd            = System.getProperty("user.home");
        this.sshClient      = null;
        this.terminalWriter = terminalWriter;
    }

    // -----------------------------------------------------------------------
    // Properties (Groovy: session.current.name / .host / .user / .cwd)
    // -----------------------------------------------------------------------

    public String getName() { return name; }
    public String getHost() { return host != null ? host : "localhost"; }
    public String getUser() { return user; }
    public String getCwd()  { return cwd; }
    public boolean isLocal(){ return sshClient == null; }

    public void updateCwd(String path) { this.cwd = path; }

    // -----------------------------------------------------------------------
    // exec — blocking, returns ExecResult
    // -----------------------------------------------------------------------

    /** Run {@code command}, return stdout/stderr/exitCode. Blocks until done. */
    public ExecResult exec(String command) throws Exception {
        return exec(command, DEFAULT_TIMEOUT_SECONDS);
    }

    public ExecResult exec(String command, int timeoutSeconds) throws Exception {
        return sshClient != null
                ? sshExec(command, timeoutSeconds)
                : localExec(command, timeoutSeconds);
    }

    /** Convenience: exec and return stdout string (throws on non-zero exit). */
    public String run(String command) throws Exception {
        ExecResult r = exec(command);
        if (!r.isSuccess())
            throw new RuntimeException("Command failed (exit " + r.exitCode() + "): " + r.stderr().trim());
        return r.output();
    }

    // -----------------------------------------------------------------------
    // send — inject text into the interactive terminal
    // -----------------------------------------------------------------------

    public void send(String text) throws IOException {
        if (terminalWriter == null) throw new IOException("No terminal attached to this session");
        terminalWriter.accept(text);
    }

    // -----------------------------------------------------------------------
    // stream — async line-by-line streaming
    // -----------------------------------------------------------------------

    /**
     * Runs {@code command} and calls {@code lineCallback} for each line of output.
     * Returns a {@link StreamHandle} immediately; streaming continues in a daemon thread.
     * Call {@link StreamHandle#stop()} to terminate.
     */
    public StreamHandle stream(String command, Consumer<String> lineCallback) {
        StreamHandle handle = new StreamHandle();
        Thread t = new Thread(() -> {
            try {
                if (sshClient != null) {
                    sshStream(command, lineCallback, handle);
                } else {
                    localStream(command, lineCallback, handle);
                }
            } catch (Exception e) {
                if (!handle.isStopped())
                    lineCallback.accept("[stream error] " + e.getMessage());
            } finally {
                handle.markDone();
            }
        }, "plugin-stream");
        t.setDaemon(true);
        t.start();
        return handle;
    }

    // -----------------------------------------------------------------------
    // SSH internals
    // -----------------------------------------------------------------------

    private ExecResult sshExec(String command, int timeoutSeconds) throws Exception {
        try (Session s = sshClient.startSession()) {
            Session.Command cmd = s.exec(command);

            ExecutorService ex = Executors.newFixedThreadPool(2);
            Future<String> stdoutF = ex.submit(() -> drain(cmd.getInputStream()));
            Future<String> stderrF = ex.submit(() -> drain(cmd.getErrorStream()));
            cmd.join(timeoutSeconds, TimeUnit.SECONDS);
            String stdout = stdoutF.get(5, TimeUnit.SECONDS);
            String stderr = stderrF.get(5, TimeUnit.SECONDS);
            ex.shutdown();

            Integer code = cmd.getExitStatus();
            return new ExecResult(stdout, stderr, code != null ? code : -1);
        }
    }

    private void sshStream(String command, Consumer<String> cb, StreamHandle handle) throws Exception {
        try (Session s = sshClient.startSession()) {
            Session.Command cmd = s.exec(command);
            handle.setProcess(null); // no local Process; stop() closes the channel via handle flag
            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(cmd.getInputStream(), StandardCharsets.UTF_8))) {
                String line;
                while (!handle.isStopped() && (line = reader.readLine()) != null) {
                    cb.accept(line);
                }
            }
            if (handle.isStopped()) cmd.close();
        }
    }

    // -----------------------------------------------------------------------
    // Local internals
    // -----------------------------------------------------------------------

    private ExecResult localExec(String command, int timeoutSeconds) throws Exception {
        boolean win = isWindows();
        ProcessBuilder pb = win
                ? new ProcessBuilder("cmd.exe", "/c", command)
                : new ProcessBuilder("/bin/sh", "-c", command);
        pb.directory(cwd != null ? new File(cwd) : new File(System.getProperty("user.home")));
        Process p = pb.start();

        ExecutorService ex = Executors.newFixedThreadPool(2);
        Future<String> stdoutF = ex.submit(() -> drain(p.getInputStream()));
        Future<String> stderrF = ex.submit(() -> drain(p.getErrorStream()));
        p.waitFor(timeoutSeconds, TimeUnit.SECONDS);
        String stdout = stdoutF.get(5, TimeUnit.SECONDS);
        String stderr = stderrF.get(5, TimeUnit.SECONDS);
        ex.shutdown();

        return new ExecResult(stdout, stderr, p.exitValue());
    }

    private void localStream(String command, Consumer<String> cb, StreamHandle handle) throws Exception {
        boolean win = isWindows();
        ProcessBuilder pb = win
                ? new ProcessBuilder("cmd.exe", "/c", command)
                : new ProcessBuilder("/bin/sh", "-c", command);
        pb.directory(cwd != null ? new File(cwd) : new File(System.getProperty("user.home")));
        Process p = pb.start();
        handle.setProcess(p);

        try (BufferedReader reader = new BufferedReader(
                new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while (!handle.isStopped() && (line = reader.readLine()) != null) {
                cb.accept(line);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Utilities
    // -----------------------------------------------------------------------

    private static String drain(InputStream is) throws IOException {
        return new String(is.readAllBytes(), StandardCharsets.UTF_8);
    }

    private static boolean isWindows() {
        return System.getProperty("os.name", "").toLowerCase().contains("win");
    }

    @Override
    public String toString() {
        return isLocal() ? "local:" + name : user + "@" + host;
    }
}
