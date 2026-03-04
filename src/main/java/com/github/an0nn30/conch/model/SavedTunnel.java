package com.github.an0nn30.conch.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.UUID;

/**
 * Persistable tunnel definition saved in config.json.
 * At activation time the matching {@link ServerEntry} is looked up
 * by host + port + username.
 */
@JsonIgnoreProperties(ignoreUnknown = true)
public class SavedTunnel {

    private String id;
    private String serverHost;
    private int    serverPort;
    private String serverUsername;
    private int    localPort;
    private String remoteHost;
    private int    remotePort;
    private String label;

    public SavedTunnel() {
        this.id = UUID.randomUUID().toString();
    }

    public SavedTunnel(String serverHost, int serverPort, String serverUsername,
                       int localPort, String remoteHost, int remotePort,
                       String label) {
        this();
        this.serverHost     = serverHost;
        this.serverPort     = serverPort;
        this.serverUsername  = serverUsername;
        this.localPort      = localPort;
        this.remoteHost     = remoteHost;
        this.remotePort     = remotePort;
        this.label = (label != null && !label.isBlank())
                ? label.strip()
                : ":" + localPort + " \u2192 " + remoteHost + ":" + remotePort;
    }

    public String getId()                          { return id; }
    public void   setId(String id)                 { this.id = id; }
    public String getServerHost()                  { return serverHost; }
    public void   setServerHost(String h)          { this.serverHost = h; }
    public int    getServerPort()                  { return serverPort; }
    public void   setServerPort(int p)             { this.serverPort = p; }
    public String getServerUsername()               { return serverUsername; }
    public void   setServerUsername(String u)       { this.serverUsername = u; }
    public int    getLocalPort()                   { return localPort; }
    public void   setLocalPort(int p)              { this.localPort = p; }
    public String getRemoteHost()                  { return remoteHost; }
    public void   setRemoteHost(String h)          { this.remoteHost = h; }
    public int    getRemotePort()                  { return remotePort; }
    public void   setRemotePort(int p)             { this.remotePort = p; }
    public String getLabel()                       { return label; }
    public void   setLabel(String label)           { this.label = label; }

    /** Returns the server descriptor used in the Via column. */
    public String serverDisplayName() {
        return serverUsername + "@" + serverHost + ":" + serverPort;
    }
}
