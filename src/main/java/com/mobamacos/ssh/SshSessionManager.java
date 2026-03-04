package com.mobamacos.ssh;

import com.mobamacos.model.ServerEntry;
import net.schmizz.sshj.SSHClient;
import net.schmizz.sshj.connection.channel.direct.PTYMode;
import net.schmizz.sshj.connection.channel.direct.Session;
import net.schmizz.sshj.transport.verification.PromiscuousVerifier;

import javax.net.SocketFactory;
import java.io.File;
import java.io.IOException;
import java.net.InetAddress;
import java.net.Socket;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

public class SshSessionManager {

    private static final int CONNECT_TIMEOUT_MS = 15_000;

    /**
     * Establishes an SSH connection and returns a JediTerm-compatible TtyConnector.
     * Supports:
     *  - Modern OpenSSH key formats (Ed25519, ECDSA, RSA "BEGIN OPENSSH PRIVATE KEY")
     *  - Password authentication
     *  - ProxyCommand (e.g. cloudflared access ssh --hostname %h)
     */
    public SshjTtyConnector connect(ServerEntry server) throws Exception {
        SSHClient ssh = connectAndAuth(server);

        // --- Open shell with PTY ---------------------------------------------
        Session session = ssh.startSession();
        session.allocatePTY("xterm-256color", 220, 50, 0, 0,
                Collections.<PTYMode, Integer>emptyMap());
        Session.Shell shell = session.startShell();

        return new SshjTtyConnector(shell, server.getName() + "@" + server.getHost(),
                server.getStartupCommand(), ssh);
    }

    /**
     * Establishes an SSH connection and authenticates, but does NOT open a shell.
     * Public so TunnelManager and RemoteFileProvider can reuse the same auth logic.
     */
    public SSHClient connectAndAuth(ServerEntry server) throws Exception {
        SSHClient ssh = new SSHClient();
        // TODO: replace PromiscuousVerifier with a proper known_hosts manager
        ssh.addHostKeyVerifier(new PromiscuousVerifier());

        // --- Connect (optionally via ProxyJump or ProxyCommand) --------------
        // ProxyCommand wins if both are set. ProxyJump is converted to the
        // equivalent "ssh -W %h:%p <jumphost>" ProxyCommand so that the
        // existing ProxyCommandSocket handles the tunnelling transparently.
        //
        // For mosh sessions, the regular proxyCommand/proxyJump fields are
        // empty (mosh handles its own bootstrap SSH connection via --ssh).
        // A dedicated moshSftpProxyCommand is used instead so that the SSHJ
        // file-transfer connection can still reach hosts behind a proxy.
        String proxyCmd  = server.getProxyCommand();
        String proxyJump = server.getProxyJump();
        if (server.isUseMosh()) {
            String sftp = server.getMoshSftpProxyCommand();
            if (sftp != null && !sftp.isBlank()) {
                proxyCmd  = sftp;
                proxyJump = null;
            }
        }
        if (proxyCmd == null || proxyCmd.isBlank()) {
            if (proxyJump != null && !proxyJump.isBlank()) {
                proxyCmd = "ssh -W %h:%p " + proxyJump;
            }
        }
        if (proxyCmd != null && !proxyCmd.isBlank()) {
            String expandedCmd = expandProxyCommand(proxyCmd, server.getHost(), server.getPort());
            ssh.setSocketFactory(new ProxyCommandSocketFactory(expandedCmd));
        } else {
            ssh.setConnectTimeout(CONNECT_TIMEOUT_MS);
        }
        ssh.connect(server.getHost(), server.getPort());

        // --- Authenticate ----------------------------------------------------
        boolean authed = false;
        String  keyPath  = server.getPrivateKeyPath();
        String  password = server.getPassword();

        if (keyPath != null && !keyPath.isBlank()) {
            try {
                ssh.authPublickey(server.getUsername(), keyPath);
                authed = true;
            } catch (Exception e) {
                System.err.println("Key auth with " + keyPath + " failed: " + e.getMessage());
            }
        }

        if (!authed) {
            List<String> defaults = defaultKeyFiles();
            if (!defaults.isEmpty()) {
                try {
                    ssh.authPublickey(server.getUsername(), defaults.toArray(new String[0]));
                    authed = true;
                } catch (Exception ignored) {}
            }
        }

        if (!authed && password != null && !password.isBlank()) {
            ssh.authPassword(server.getUsername(), password);
            authed = true;
        }

        if (!authed) {
            ssh.disconnect();
            throw new Exception("All authentication methods failed for "
                    + server.getUsername() + "@" + server.getHost());
        }

        return ssh;
    }

    // -------------------------------------------------------------------------

    private static String expandProxyCommand(String template, String host, int port) {
        return template
                .replace("%h", host)
                .replace("%p", String.valueOf(port));
    }

    private static List<String> defaultKeyFiles() {
        String home = System.getProperty("user.home");
        String[] candidates = {
                home + "/.ssh/id_ed25519",
                home + "/.ssh/id_ecdsa",
                home + "/.ssh/id_rsa",
                home + "/.ssh/id_dsa"
        };
        List<String> found = new ArrayList<>();
        for (String path : candidates) {
            if (new File(path).exists()) found.add(path);
        }
        return found;
    }

    // -------------------------------------------------------------------------
    // Inner SocketFactory that produces ProxyCommandSocket instances
    // -------------------------------------------------------------------------

    private static class ProxyCommandSocketFactory extends SocketFactory {
        private final String command;

        ProxyCommandSocketFactory(String command) { this.command = command; }

        @Override
        public Socket createSocket() { return new ProxyCommandSocket(command); }

        @Override
        public Socket createSocket(String host, int port) throws IOException {
            // Create + immediately connect (some callers skip the separate connect step)
            ProxyCommandSocket s = new ProxyCommandSocket(command);
            s.connect(new java.net.InetSocketAddress(host, port));
            return s;
        }

        @Override
        public Socket createSocket(String host, int port, InetAddress localHost, int localPort)
                throws IOException { return createSocket(host, port); }

        @Override
        public Socket createSocket(InetAddress host, int port) throws IOException {
            return createSocket(host.getHostName(), port);
        }

        @Override
        public Socket createSocket(InetAddress address, int port,
                                   InetAddress localAddress, int localPort) throws IOException {
            return createSocket(address.getHostName(), port);
        }
    }
}
