package com.github.an0nn30.conch.ssh;

import com.github.an0nn30.conch.model.SavedTunnel;
import com.github.an0nn30.conch.model.ServerEntry;
import net.schmizz.sshj.SSHClient;
import net.schmizz.sshj.connection.channel.direct.LocalPortForwarder;
import net.schmizz.sshj.connection.channel.direct.Parameters;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public class TunnelManager {

    private final SshSessionManager         sessionManager = new SshSessionManager();
    private final Map<String, ActiveTunnel> activeTunnels  = new ConcurrentHashMap<>();

    /**
     * Activates a saved tunnel definition by establishing an SSH connection
     * and creating a local port forward.
     *
     * @param saved  the saved tunnel definition
     * @param server the ServerEntry to connect through
     * @throws Exception if the SSH connection or port binding fails
     */
    public void activate(SavedTunnel saved, ServerEntry server) throws Exception {
        if (isActive(saved)) return;

        SSHClient ssh = sessionManager.connectAndAuth(server);

        Parameters params = new Parameters(
                "127.0.0.1", saved.getLocalPort(),
                saved.getRemoteHost(), saved.getRemotePort());

        ServerSocket ss = new ServerSocket();
        ss.setReuseAddress(true);
        ss.bind(new InetSocketAddress(
                InetAddress.getByName("127.0.0.1"), saved.getLocalPort()));

        LocalPortForwarder forwarder = ssh.newLocalPortForwarder(params, ss);

        ActiveTunnel tunnel = new ActiveTunnel(saved, ssh, ss, forwarder);
        activeTunnels.put(saved.getId(), tunnel);
    }

    /** Deactivates (stops) a tunnel but keeps its saved definition. */
    public void deactivate(SavedTunnel saved) {
        ActiveTunnel tunnel = activeTunnels.remove(saved.getId());
        if (tunnel != null) tunnel.stop();
    }

    /** Returns true if the tunnel is currently active and connected. */
    public boolean isActive(SavedTunnel saved) {
        ActiveTunnel t = activeTunnels.get(saved.getId());
        return t != null && t.isRunning();
    }

    // -----------------------------------------------------------------------

    public static class ActiveTunnel {

        private final SavedTunnel  saved;
        private final SSHClient    ssh;
        private final ServerSocket serverSocket;
        private volatile boolean   stopped = false;

        ActiveTunnel(SavedTunnel saved, SSHClient ssh,
                     ServerSocket serverSocket, LocalPortForwarder forwarder) {
            this.saved        = saved;
            this.ssh          = ssh;
            this.serverSocket = serverSocket;

            Thread t = new Thread(() -> {
                try {
                    forwarder.listen();
                } catch (Exception e) {
                    if (!stopped) {
                        System.err.println("Tunnel \"" + saved.getLabel()
                                + "\" dropped: " + e.getMessage());
                    }
                }
                stopped = true;
            }, "tunnel-" + saved.getLocalPort());
            t.setDaemon(true);
            t.start();
        }

        void stop() {
            stopped = true;
            try { serverSocket.close(); } catch (IOException ignored) {}
            try { ssh.disconnect();    } catch (IOException ignored) {}
        }

        public boolean     isRunning() { return !stopped && ssh.isConnected(); }
        public SavedTunnel getSaved()  { return saved; }
    }
}
