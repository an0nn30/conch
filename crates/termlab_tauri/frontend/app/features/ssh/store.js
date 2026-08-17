(function initTermLabSshStore(global) {
  'use strict';

  function getAllServers(serverData) {
    const all = [];
    const data = serverData || { folders: [], ungrouped: [], ssh_config: [] };
    for (const folder of data.folders || []) {
      for (const server of folder.entries || []) all.push(server);
    }
    for (const server of data.ungrouped || []) all.push(server);
    for (const server of data.ssh_config || []) all.push(server);
    return all;
  }

  function serverMatchesQuery(server, query) {
    if (!query) return true;
    const s = server || {};
    const hay = `${s.label || ''} ${s.host || ''} ${s.user || ''}@${s.host || ''}`.toLowerCase();
    return String(query).split(/\s+/).every((term) => hay.includes(term));
  }

  function getFilteredServers(serverData, query) {
    if (!query) return [];
    return getAllServers(serverData).filter((server) => serverMatchesQuery(server, query));
  }

  function parseProxyJump(value) {
    const raw = String(value || '').trim();
    if (!raw) return null;
    const match = raw.match(/^(?:(.+?)@)?(\[[^\]]+\]|[^:]+?)(?::(\d+))?$/);
    if (!match) return null;
    const user = (match[1] || '').trim();
    const host = (match[2] || '').trim().toLowerCase();
    if (!host) return null;
    const port = match[3] ? parseInt(match[3], 10) : 22;
    return { user: user.toLowerCase(), host, port: Number.isFinite(port) ? port : 22 };
  }

  function normalizeProxyJump(value) {
    const parsed = parseProxyJump(value);
    if (!parsed) return null;
    return `${parsed.user}@${parsed.host}:${parsed.port}`;
  }

  function makeProxyJumpSpec(server) {
    if (!server || !server.host) return '';
    const host = String(server.host).trim();
    if (!host) return '';
    const user = String(server.user || '').trim();
    const port = Number.isFinite(Number(server.port)) ? Number(server.port) : 22;
    const base = user ? `${user}@${host}` : host;
    return port === 22 ? base : `${base}:${port}`;
  }

  function findServerForProxyJump(proxyJumpValue, servers) {
    const parsed = parseProxyJump(proxyJumpValue);
    if (!parsed) return null;
    const list = Array.isArray(servers) ? servers : [];
    const normalized = normalizeProxyJump(proxyJumpValue);
    if (parsed.user) {
      return list.find((server) => normalizeProxyJump(makeProxyJumpSpec(server)) === normalized) || null;
    }
    return list.find((server) => {
      const spec = parseProxyJump(makeProxyJumpSpec(server));
      return spec && spec.host === parsed.host && spec.port === parsed.port;
    }) || null;
  }

  function buildProxyJumpOptions(serverData, excludedServerId) {
    const options = [];
    const seenSpecs = new Set();
    const data = serverData || { folders: [], ungrouped: [], ssh_config: [] };

    const addFromList = (servers, source) => {
      for (const server of servers || []) {
        if (server.id === excludedServerId) continue;
        const spec = makeProxyJumpSpec(server);
        if (!spec) continue;
        const normalizedSpec = normalizeProxyJump(spec);
        if (!normalizedSpec || seenSpecs.has(normalizedSpec)) continue;
        seenSpecs.add(normalizedSpec);
        options.push({
          source,
          spec,
          label: server.label || spec,
          details: `${server.user || 'user'}@${server.host}:${server.port || 22}`,
        });
      }
    };

    for (const folder of data.folders || []) addFromList(folder.entries, 'saved');
    addFromList(data.ungrouped, 'saved');
    addFromList(data.ssh_config, 'ssh_config');

    return options;
  }

  function renderProxyJumpOptions(options, deps) {
    const esc = deps && typeof deps.esc === 'function'
      ? deps.esc
      : (value) => String(value == null ? '' : value);
    const attr = deps && typeof deps.attr === 'function'
      ? deps.attr
      : esc;
    const groups = [
      { source: 'saved', title: 'Saved Sessions' },
      { source: 'ssh_config', title: '~/.ssh/config' },
    ];
    return groups
      .map((group) => {
        const groupOptions = (options || []).filter((opt) => opt.source === group.source);
        if (!groupOptions.length) return '';
        const optionHtml = groupOptions
          .map((opt) => `<option value="${attr(opt.spec)}">${esc(opt.label)} (${esc(opt.details)})</option>`)
          .join('');
        return `<optgroup label="${esc(group.title)}">${optionHtml}</optgroup>`;
      })
      .join('');
  }

  /** Parse a `user@host:port` session key the same way the backend's
   * `SavedTunnel::parse_session_key` does: split on the first `@`, then the
   * *last* `:` (so an IPv6 host containing `:` still yields the trailing
   * port). Returns null for anything that doesn't fit that shape. */
  function parseSessionKey(sessionKey) {
    const raw = String(sessionKey || '');
    const atIdx = raw.indexOf('@');
    if (atIdx === -1) return null;
    const user = raw.slice(0, atIdx);
    const rest = raw.slice(atIdx + 1);
    const colonIdx = rest.lastIndexOf(':');
    if (colonIdx === -1) return null;
    const host = rest.slice(0, colonIdx);
    const port = parseInt(rest.slice(colonIdx + 1), 10);
    if (!host || !Number.isFinite(port)) return null;
    return { user, host, port };
  }

  /** Resolve a tunnel's dependency host using the same precedence the
   * backend's `export_planner::resolve_tunnel_host` uses: `server_entry_id`
   * first (an exact id match against everything visible — folders,
   * ungrouped, and ~/.ssh/config aliases, in that order, matching how
   * `servers` here is already assembled), then `session_key` parsed and
   * matched by host+port only — never by `user`, and never by exact string
   * equality against the whole session key. The previous implementation
   * compared `user@host:port` strings verbatim, which silently failed to
   * find the dependency (so the user was never shown it, and never asked)
   * whenever a tunnel's `session_key` had gone stale after its host's user
   * was edited, or whenever the host had no `user` at all (common for
   * ~/.ssh/config aliases, which produced the literal string
   * "undefined@host:22"). Mirroring the backend's resolution here is what
   * keeps the dependency prompt from ever disagreeing with what the
   * planner actually pulls in (2026-08-16 review finding I3). */
  function findServerForTunnel(tunnel, servers) {
    const list = Array.isArray(servers) ? servers : [];
    const t = tunnel || {};
    if (t.server_entry_id) {
      const byId = list.find((s) => s.id === t.server_entry_id);
      if (byId) return byId;
    }
    const parsed = parseSessionKey(t.session_key);
    if (!parsed) return null;
    return list.find((s) => s.host === parsed.host && Number(s.port) === parsed.port) || null;
  }

  function dedupeDependencyServers(missingDependencies) {
    const seen = new Set();
    const deduped = [];
    for (const dep of missingDependencies || []) {
      const key = `${dep.reason}:${dep.sourceId}:${dep.server.id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      deduped.push(dep);
    }
    return deduped;
  }

  global.termlabSshStore = {
    getAllServers,
    serverMatchesQuery,
    getFilteredServers,
    parseProxyJump,
    normalizeProxyJump,
    makeProxyJumpSpec,
    findServerForProxyJump,
    buildProxyJumpOptions,
    renderProxyJumpOptions,
    dedupeDependencyServers,
    parseSessionKey,
    findServerForTunnel,
  };
})(window);
