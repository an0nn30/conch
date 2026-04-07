(function initConchSshStore(global) {
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

  global.conchSshStore = {
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
  };
})(window);
