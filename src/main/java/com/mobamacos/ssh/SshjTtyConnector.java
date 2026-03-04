package com.mobamacos.ssh;

import com.jediterm.core.util.TermSize;
import com.jediterm.terminal.TtyConnector;
import net.schmizz.sshj.SSHClient;
import net.schmizz.sshj.connection.channel.direct.Session;
import org.jetbrains.annotations.NotNull;

import java.io.*;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;

/**
 * Bridges SSHJ's Session.Shell to JediTerm's TtyConnector interface.
 *
 * Also intercepts OSC 7 escape sequences emitted by the remote shell
 * (format: ESC ] 7 ; file://hostname/path BEL) and fires a {@link CwdListener}
 * whenever the shell reports a new working directory.
 *
 * At session start, a compact shell-integration snippet is injected so that
 * bash and zsh automatically emit OSC 7 on every prompt — even if the user's
 * shell is not pre-configured to do so.  Fish emits OSC 7 natively.
 */
public class SshjTtyConnector implements TtyConnector {

    // -----------------------------------------------------------------------
    // CWD listener
    // -----------------------------------------------------------------------

    public interface CwdListener {
        /** Called (off the EDT) whenever the remote shell reports a new CWD. */
        void cwdChanged(String absolutePath);
    }

    // -----------------------------------------------------------------------
    // Fields
    // -----------------------------------------------------------------------

    private final Session.Shell shell;
    private final Reader        reader;
    private final OutputStream  outputStream;
    private final String        name;
    private final String        startupCommand;
    private final SSHClient     sshClient;

    private volatile CwdListener cwdListener;

    /** State machine for OSC 7 detection. */
    private final OscParser oscParser = new OscParser();

    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    public SshjTtyConnector(Session.Shell shell, String name, String startupCommand,
                             SSHClient sshClient) throws IOException {
        this.shell          = shell;
        this.outputStream   = shell.getOutputStream();
        this.reader         = new InputStreamReader(shell.getInputStream(), StandardCharsets.UTF_8);
        this.name           = name;
        this.startupCommand = startupCommand != null ? startupCommand : "";
        this.sshClient      = sshClient;
        injectShellIntegration();
    }

    public void setCwdListener(CwdListener l) { this.cwdListener = l; }

    /** Returns the underlying {@link SSHClient} for plugin exec commands. */
    public SSHClient getSshClient() { return sshClient; }

    // -----------------------------------------------------------------------
    // TtyConnector
    // -----------------------------------------------------------------------

    @Override
    public int read(char[] buf, int offset, int length) throws IOException {
        int n = reader.read(buf, offset, length);
        if (n > 0) oscParser.process(buf, offset, n);
        return n;
    }

    @Override
    public void write(byte[] bytes) throws IOException {
        outputStream.write(bytes);
        outputStream.flush();
    }

    @Override
    public void write(String string) throws IOException {
        write(string.getBytes(StandardCharsets.UTF_8));
    }

    @Override public boolean isConnected() { return shell.isOpen(); }

    @Override
    public void resize(@NotNull TermSize termSize) {
        try { shell.changeWindowDimensions(termSize.getColumns(), termSize.getRows(), 0, 0); }
        catch (Exception ignored) {}
    }

    @Override
    public int waitFor() throws InterruptedException {
        while (shell.isOpen()) Thread.sleep(200);
        return 0;
    }

    @Override public boolean ready() throws IOException { return reader.ready(); }
    @Override public String  getName()                  { return name; }

    @Override
    public void close() {
        try { shell.close(); } catch (Exception ignored) {}
    }

    // -----------------------------------------------------------------------
    // Shell integration injection
    // -----------------------------------------------------------------------

    /**
     * Injects a minimal PROMPT_COMMAND / precmd hook that makes bash and zsh
     * emit an OSC 7 sequence on every prompt.  Fish emits OSC 7 natively.
     *
     * The command is sent 400 ms after construction to give the shell time to
     * finish its own initialisation scripts (.bashrc / .zshrc).
     *
     * The leading space keeps it out of bash history when
     * HISTCONTROL=ignorespace is set (common default).
     */
    private void injectShellIntegration() {
        Thread t = new Thread(() -> {
            try {
                Thread.sleep(400);
                // Disable local echo so the integration script isn't printed on screen.
                // Only "stty -echo" itself is briefly visible; the printf at the end
                // erases it (moves up one line, clears, returns to start).
                write("stty -echo\r");
                Thread.sleep(100); // let stty take effect before sending the script
                String cmd =
                    " __mc(){ printf '\\033]7;file://%s\\007' \"$PWD\"; };" +
                    " [ -n \"$BASH_VERSION\" ] && PROMPT_COMMAND=\"${PROMPT_COMMAND:+$PROMPT_COMMAND;}__mc\";" +
                    " [ -n \"$ZSH_VERSION\" ]  && precmd_functions+=(__mc);" +
                    " __mc;" +
                    " stty echo;" +
                    " printf '\\033[2K\\033[1A\\033[2K\\r'\r";
                write(cmd);
                if (!startupCommand.isBlank()) {
                    Thread.sleep(100);
                    write(startupCommand.trim() + "\r");
                }
            } catch (Exception ignored) {}
        }, "shell-integration-injector");
        t.setDaemon(true);
        t.start();
    }

    // -----------------------------------------------------------------------
    // OSC 7 state machine
    // -----------------------------------------------------------------------

    private class OscParser {
        private enum State { NORMAL, ESC, OSC_NUM, OSC_7_DATA, ST_WAIT }

        private State         state  = State.NORMAL;
        private int           oscNum = 0;
        private StringBuilder buf    = new StringBuilder();

        void process(char[] data, int off, int len) {
            for (int i = off; i < off + len; i++) {
                char c = data[i];
                switch (state) {
                    case NORMAL   -> { if (c == '\u001B') state = State.ESC; }
                    case ESC      -> {
                        if (c == ']') { state = State.OSC_NUM; oscNum = 0; buf.setLength(0); }
                        else           state = State.NORMAL;
                    }
                    case OSC_NUM  -> {
                        if (c >= '0' && c <= '9') { oscNum = oscNum * 10 + (c - '0'); }
                        else if (c == ';') {
                            if (oscNum == 7) state = State.OSC_7_DATA;
                            else             state = State.NORMAL;
                        } else state = State.NORMAL;
                    }
                    case OSC_7_DATA -> {
                        if (c == '\u0007') { fire(buf.toString()); state = State.NORMAL; }
                        else if (c == '\u001B') state = State.ST_WAIT;
                        else buf.append(c);
                    }
                    case ST_WAIT -> {
                        if (c == '\\') fire(buf.toString());
                        state = State.NORMAL;
                    }
                }
            }
        }

        private void fire(String url) {
            CwdListener l = cwdListener;
            if (l == null || !url.startsWith("file://")) return;
            // file://hostname/path  or  file:///path  →  extract the path part
            String rest = url.substring(7);            // drop "file://"
            String path = rest.startsWith("/") ? rest  // already starts with /path
                        : rest.contains("/") ? rest.substring(rest.indexOf('/'))
                        : null;
            if (path == null || path.isBlank()) return;
            try { path = URLDecoder.decode(path, StandardCharsets.UTF_8); }
            catch (Exception ignored) {}
            l.cwdChanged(path);
        }
    }
}
