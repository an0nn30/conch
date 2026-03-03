package com.mobamacos.terminal;

import com.jediterm.core.util.TermSize;
import com.jediterm.terminal.TtyConnector;
import com.pty4j.PtyProcess;
import com.pty4j.PtyProcessBuilder;
import com.pty4j.WinSize;
import org.jetbrains.annotations.NotNull;

import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;

/**
 * TtyConnector that opens a proper local PTY shell (zsh/bash) via pty4j.
 * Supports resize, readline, job-control, colours — everything a real terminal needs.
 */
public class LocalShellTtyConnector implements TtyConnector {

    private final PtyProcess  pty;
    private final Reader      reader;
    private final OutputStream writer;

    public LocalShellTtyConnector() throws IOException {
        boolean windows = System.getProperty("os.name", "").toLowerCase().contains("win");

        Map<String, String> env = new HashMap<>(System.getenv());
        String[] cmd;

        if (windows) {
            // On Windows, pty4j uses ConPTY (Windows 10 1903+) to host the shell.
            // $SHELL is not a Windows env var; use PowerShell 5.x which is
            // pre-installed on every Windows 10/11 machine.
            // -NoExit keeps the session alive; -NoLogo suppresses the banner.
            cmd = new String[]{"powershell.exe", "-NoExit", "-NoLogo"};
        } else {
            String shell = System.getenv("SHELL");
            if (shell == null || shell.isBlank()) shell = "/bin/zsh";

            env.put("TERM",      "xterm-256color");
            env.put("COLORTERM", "truecolor");

            // --login so ~/.zshrc / ~/.bash_profile are sourced
            cmd = (shell.endsWith("bash") || shell.endsWith("zsh"))
                    ? new String[]{shell, "--login"}
                    : new String[]{shell};
        }

        pty = new PtyProcessBuilder(cmd)
                .setEnvironment(env)
                .setDirectory(System.getProperty("user.home"))
                .setInitialColumns(220)
                .setInitialRows(50)
                .start();

        reader = new InputStreamReader(pty.getInputStream(), StandardCharsets.UTF_8);
        writer = pty.getOutputStream();
    }

    @Override public int     read(char[] buf, int offset, int length) throws IOException { return reader.read(buf, offset, length); }
    @Override public boolean ready()      throws IOException  { return reader.ready(); }
    @Override public boolean isConnected()                    { return pty.isAlive(); }
    @Override public String  getName()                        { return "Local Terminal"; }

    @Override
    public void write(byte[] bytes) throws IOException {
        writer.write(bytes);
        writer.flush();
    }

    @Override
    public void write(String s) throws IOException { write(s.getBytes(StandardCharsets.UTF_8)); }

    @Override
    public void resize(@NotNull TermSize termSize) {
        pty.setWinSize(new WinSize(termSize.getColumns(), termSize.getRows()));
    }

    @Override
    public int waitFor() throws InterruptedException { return pty.waitFor(); }

    @Override
    public void close() { pty.destroy(); }
}
