package com.mobamacos.model;

import com.fasterxml.jackson.annotation.JsonIgnoreProperties;
import java.util.UUID;

@JsonIgnoreProperties(ignoreUnknown = true)
public class ServerEntry {
    private String id;
    private String name;
    private String host;
    private int port = 22;
    private String username;
    private String password = "";
    private String privateKeyPath = "";
    /** ProxyCommand, e.g. "cloudflared access ssh --hostname %h" or "ssh -W %h:%p bastion" */
    private String proxyCommand = "";
    /** ProxyJump host, e.g. "user@bastion.example.com" or "bastion:2222" */
    private String proxyJump = "";
    /** True when imported from ~/.ssh/config — never persisted back to that file */
    private boolean fromSshConfig = false;

    public ServerEntry() {
        this.id = UUID.randomUUID().toString();
    }

    public ServerEntry(String name, String host, int port, String username) {
        this();
        this.name = name;
        this.host = host;
        this.port = port;
        this.username = username;
    }

    public String getId()                      { return id; }
    public void   setId(String id)             { this.id = id; }
    public String getName()                    { return name; }
    public void   setName(String name)         { this.name = name; }
    public String getHost()                    { return host; }
    public void   setHost(String host)         { this.host = host; }
    public int    getPort()                    { return port; }
    public void   setPort(int port)            { this.port = port; }
    public String getUsername()                { return username; }
    public void   setUsername(String username) { this.username = username; }
    public String getPassword()                { return password; }
    public void   setPassword(String password) { this.password = password; }
    public String getPrivateKeyPath()          { return privateKeyPath; }
    public void   setPrivateKeyPath(String p)  { this.privateKeyPath = p; }
    public String getProxyCommand()              { return proxyCommand; }
    public void   setProxyCommand(String cmd)   { this.proxyCommand = cmd; }
    public String getProxyJump()                { return proxyJump; }
    public void   setProxyJump(String jump)     { this.proxyJump = jump; }
    public boolean isFromSshConfig()            { return fromSshConfig; }
    public void    setFromSshConfig(boolean b)  { this.fromSshConfig = b; }

    @Override
    public String toString() {
        return name + " (" + host + ":" + port + ")";
    }
}
