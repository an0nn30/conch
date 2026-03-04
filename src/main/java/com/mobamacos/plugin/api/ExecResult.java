package com.mobamacos.plugin.api;

/** Result of a non-interactive command execution. */
public record ExecResult(String stdout, String stderr, int exitCode) {
    public boolean isSuccess() { return exitCode == 0; }

    /** Convenience: stdout trimmed. */
    public String output() { return stdout.trim(); }

    @Override
    public String toString() { return stdout; }
}
