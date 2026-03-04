package com.mobamacos.plugin.api;

import java.util.concurrent.CountDownLatch;

/**
 * Handle returned by {@code session.stream()}.
 * Call {@link #stop()} to terminate the stream, {@link #waitFor()} to block until it ends.
 */
public class StreamHandle {

    private volatile boolean stopped = false;
    private final CountDownLatch done = new CountDownLatch(1);
    private volatile Process process;   // set by PluginSession after process starts

    public void stop() {
        stopped = true;
        if (process != null) process.destroy();
    }

    public void waitFor() throws InterruptedException { done.await(); }

    // ---- package-private, used by PluginSession ----
    boolean isStopped()          { return stopped; }
    void    setProcess(Process p){ this.process = p; }
    void    markDone()           { done.countDown(); }
}
