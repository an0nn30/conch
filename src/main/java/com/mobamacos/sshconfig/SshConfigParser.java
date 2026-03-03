package com.mobamacos.sshconfig;

import com.mobamacos.model.ServerEntry;
import com.mobamacos.model.ServerFolder;

import java.io.BufferedReader;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * Parses ~/.ssh/config and converts named Host entries into ServerEntry objects.
 * Wildcard entries (Host *) are skipped. ProxyCommand directives are preserved.
 */
public class SshConfigParser {

    private static final Path SSH_CONFIG = Paths.get(System.getProperty("user.home"), ".ssh", "config");

    public static ServerFolder parse() {
        ServerFolder folder = new ServerFolder("SSH Config");
        if (!Files.exists(SSH_CONFIG)) {
            return folder;
        }

        try (BufferedReader reader = Files.newBufferedReader(SSH_CONFIG)) {
            String alias        = null;
            String hostname     = null;
            String user         = System.getProperty("user.name");
            int    port         = 22;
            String identityFile = "";
            String proxyCommand = "";
            String proxyJump    = "";

            String line;
            while ((line = reader.readLine()) != null) {
                line = line.strip();
                if (line.isEmpty() || line.startsWith("#")) continue;

                String lower = line.toLowerCase();

                if (lower.startsWith("host ")) {
                    // Commit the previous block
                    if (alias != null && !alias.contains("*") && hostname != null) {
                        folder.getServers().add(buildEntry(alias, hostname, port, user,
                                identityFile, proxyCommand, proxyJump));
                    }
                    // Start a new block — a Host line can have multiple aliases separated by spaces;
                    // we use the first one as the display name.
                    String aliasStr = line.substring(5).strip();
                    alias        = aliasStr.split("\\s+")[0];
                    hostname     = alias;   // default: HostName == first alias
                    user         = System.getProperty("user.name");
                    port         = 22;
                    identityFile = "";
                    proxyCommand = "";
                    proxyJump    = "";

                } else if (lower.startsWith("hostname ")) {
                    hostname = line.substring(9).strip();
                } else if (lower.startsWith("user ")) {
                    user = line.substring(5).strip();
                } else if (lower.startsWith("port ")) {
                    try { port = Integer.parseInt(line.substring(5).strip()); }
                    catch (NumberFormatException ignored) {}
                } else if (lower.startsWith("identityfile ")) {
                    identityFile = line.substring(13).strip();
                } else if (lower.startsWith("proxycommand ")) {
                    proxyCommand = line.substring(13).strip();
                } else if (lower.startsWith("proxyjump ")) {
                    proxyJump = line.substring(10).strip();
                }
            }

            // Commit the last block
            if (alias != null && !alias.contains("*") && hostname != null) {
                folder.getServers().add(buildEntry(alias, hostname, port, user,
                        identityFile, proxyCommand, proxyJump));
            }

        } catch (IOException e) {
            System.err.println("Could not parse ~/.ssh/config: " + e.getMessage());
        }

        return folder;
    }

    private static ServerEntry buildEntry(String alias, String hostname, int port,
                                          String user, String identityFile,
                                          String proxyCommand, String proxyJump) {
        ServerEntry entry = new ServerEntry(alias, hostname, port, user);
        entry.setPrivateKeyPath(expandTilde(identityFile));
        entry.setProxyCommand(proxyCommand);
        entry.setProxyJump(proxyJump);
        entry.setFromSshConfig(true);
        return entry;
    }

    private static String expandTilde(String path) {
        if (path == null || path.isEmpty()) return "";
        return path.startsWith("~")
                ? System.getProperty("user.home") + path.substring(1)
                : path;
    }
}
