(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let panelEl = null;
  let sessions = [];
  let selectedSessionName = null;
  let connectedSessionName = null;
  let filterText = '';
  let autoCreateInFlight = false;
  let autoAttachInFlight = false;
  let initialAutoAttachAttempted = false;

  function confirmKillSession(name) {
    return new Promise(function (resolve) {
      var overlay = document.createElement('div');
      overlay.className = 'ssh-overlay tmux-kill-confirm-overlay';
      overlay.innerHTML = [
        '<div class="ssh-form ssh-form-small tmux-kill-confirm" role="dialog" aria-modal="true">',
        '  <div class="ssh-form-title">Kill tmux session?</div>',
        '  <div class="ssh-form-body">',
        '    <div class="tmux-kill-confirm-copy">This will permanently end "' + escapeHtml(name) + '".</div>',
        '  </div>',
        '  <div class="ssh-form-buttons">',
        '    <button type="button" class="ssh-form-btn" data-role="cancel">Cancel</button>',
        '    <button type="button" class="ssh-form-btn tmux-kill-confirm-danger" data-role="confirm">Kill Session</button>',
        '  </div>',
        '</div>',
      ].join('');
      function close(result) {
        overlay.remove();
        resolve(result);
      }
      overlay.addEventListener('click', function (event) {
        if (event.target === overlay) close(false);
      });
      overlay.querySelector('[data-role="cancel"]').addEventListener('click', function () { close(false); });
      overlay.querySelector('[data-role="confirm"]').addEventListener('click', function () { close(true); });
      document.body.appendChild(overlay);
      overlay.querySelector('[data-role="confirm"]').focus();
    });
  }

  function promptRenameSession(currentName) {
    return new Promise(function (resolve) {
      var overlay = document.createElement('div');
      overlay.className = 'ssh-overlay tmux-rename-overlay';
      overlay.innerHTML = [
        '<div class="ssh-form ssh-form-small tmux-rename-form" role="dialog" aria-modal="true">',
        '  <div class="ssh-form-title">Rename tmux session</div>',
        '  <div class="ssh-form-body">',
        '    <label class="ssh-form-label">',
        '      New name',
        '      <input type="text" data-role="name" autocomplete="off" value="' + escapeAttr(currentName) + '" />',
        '    </label>',
        '  </div>',
        '  <div class="ssh-form-buttons">',
        '    <button type="button" class="ssh-form-btn" data-role="cancel">Cancel</button>',
        '    <button type="button" class="ssh-form-btn primary" data-role="confirm">Rename</button>',
        '  </div>',
        '</div>',
      ].join('');

      var input = null;
      function close(result) {
        overlay.remove();
        resolve(result);
      }
      function confirm() {
        var value = input ? String(input.value || '').trim() : '';
        if (!value || value === currentName) {
          close(null);
          return;
        }
        close(value);
      }

      overlay.addEventListener('click', function (event) {
        if (event.target === overlay) close(null);
      });
      overlay.querySelector('[data-role="cancel"]').addEventListener('click', function () { close(null); });
      overlay.querySelector('[data-role="confirm"]').addEventListener('click', function () { confirm(); });
      document.body.appendChild(overlay);
      input = overlay.querySelector('[data-role="name"]');
      if (input) {
        input.focus();
        input.select();
        input.addEventListener('keydown', function (event) {
          if (event.key === 'Enter') {
            event.preventDefault();
            confirm();
          } else if (event.key === 'Escape') {
            event.preventDefault();
            close(null);
          }
        });
      }
    });
  }

  function init(opts) {
    invoke = opts.invoke;
    listen = opts.listen;
    panelEl = opts.panelEl;
    render();
    bindEvents();
    refreshSessions();
  }

  function render() {
    if (!panelEl) return;
    panelEl.innerHTML = [
      '<section class="tmux-browser">',
      '  <header class="tmux-browser-header">',
      '    <div>',
      '      <div class="tmux-browser-eyebrow">Workspace Backend</div>',
      '      <h2 class="tmux-browser-title">Tmux Sessions</h2>',
      '      <p class="tmux-browser-copy">Attach, create, and manage tmux workspaces.</p>',
      '    </div>',
      '    <div class="tmux-browser-actions">',
      '      <button class="tmux-action-btn tmux-action-btn-primary" data-action="new">New Session</button>',
      '      <button class="tmux-action-btn" data-action="refresh">Refresh</button>',
      '    </div>',
      '  </header>',
      '  <div class="tmux-browser-toolbar">',
      '    <input class="tmux-search-input" data-role="search" type="search" placeholder="Search sessions" />',
      '    <div class="tmux-browser-meta" data-role="meta"></div>',
      '  </div>',
      '  <div class="tmux-session-cards" data-role="session-list"></div>',
      '</section>',
    ].join('');
    renderSessionList();
  }

  function getFilteredSessions() {
    if (!filterText) return sessions.slice();
    var query = filterText.toLowerCase();
    return sessions.filter(function (session) {
      return String(session.name || '').toLowerCase().includes(query);
    });
  }

  function renderMeta(filteredCount) {
    var metaEl = panelEl && panelEl.querySelector('[data-role="meta"]');
    if (!metaEl) return;
    var bits = [];
    bits.push(filteredCount + (filteredCount === 1 ? ' session' : ' sessions'));
    if (connectedSessionName) {
      bits.push('Attached to ' + connectedSessionName);
    }
    metaEl.textContent = bits.join(' • ');
  }

  function renderSessionList() {
    if (!panelEl) return;
    var listEl = panelEl.querySelector('[data-role="session-list"]');
    if (!listEl) return;

    var filtered = getFilteredSessions();
    renderMeta(filtered.length);

    if (filtered.length === 0) {
      listEl.innerHTML = [
        '<div class="tmux-empty-state">',
        '  <div class="tmux-empty-title">' + (filterText ? 'No matching sessions' : 'No tmux sessions found') + '</div>',
        '  <div class="tmux-empty-copy">' + (filterText ? 'Try a different search term or create a new session.' : 'Create a session to start using tmux-backed tabs and panes.') + '</div>',
        '  <button class="tmux-action-btn tmux-action-btn-primary" data-action="new">Create Session</button>',
        '</div>',
      ].join('');
      return;
    }

    listEl.innerHTML = filtered.map(function (session) {
      var selected = session.name === selectedSessionName ? ' tmux-session-card-selected' : '';
      var connected = session.name === connectedSessionName ? ' tmux-session-card-connected' : '';
      var attached = session.attached ? '<span class="tmux-session-chip">Attached</span>' : '<span class="tmux-session-chip tmux-session-chip-muted">Detached</span>';
      var connectedChip = session.name === connectedSessionName ? '<span class="tmux-session-chip tmux-session-chip-primary">Current</span>' : '';
      var windowCount = Number(session.window_count) || 0;
      return [
        '<article class="tmux-session-card' + selected + connected + '" data-session="' + escapeAttr(session.name) + '">',
        '  <div class="tmux-session-main">',
        '    <div class="tmux-session-topline">',
        '      <div class="tmux-session-name">' + escapeHtml(session.name) + '</div>',
        '      <div class="tmux-session-chips">' + attached + connectedChip + '</div>',
        '    </div>',
        '    <div class="tmux-session-subline">',
        '      <span>' + windowCount + (windowCount === 1 ? ' window' : ' windows') + '</span>',
        '      <span>Session $' + Number(session.id || 0) + '</span>',
        '    </div>',
        '  </div>',
        '  <div class="tmux-session-row-actions">',
        '    <button class="tmux-inline-btn" data-action="attach" data-session="' + escapeAttr(session.name) + '">Attach</button>',
        '    <button class="tmux-inline-btn" data-action="rename" data-session="' + escapeAttr(session.name) + '">Rename</button>',
        '    <button class="tmux-inline-btn tmux-inline-btn-danger" data-action="kill" data-session="' + escapeAttr(session.name) + '">Kill</button>',
        '  </div>',
        '</article>',
      ].join('');
    }).join('');
  }

  function bindEvents() {
    if (!panelEl) return;

    panelEl.addEventListener('input', function (event) {
      var search = event.target.closest('[data-role="search"]');
      if (!search) return;
      filterText = String(search.value || '').trim();
      renderSessionList();
    });

    panelEl.addEventListener('click', function (event) {
      console.info('[tmux] panel click', {
        targetTag: event.target && event.target.tagName,
        targetClass: event.target && event.target.className,
      });
      var actionEl = event.target.closest('[data-action]');
      if (actionEl) {
        var action = actionEl.dataset.action;
        var sessionName = actionEl.dataset.session || selectedSessionName;
        console.info('[tmux] panel action', {
          action: action,
          sessionName: sessionName || null,
        });
        if (action === 'new') createSession();
        else if (action === 'refresh') refreshSessions();
        else if (action === 'attach' && sessionName) attachSession(sessionName);
        else if (action === 'rename' && sessionName) renameSession(sessionName);
        else if (action === 'kill' && sessionName) killSession(sessionName);
        return;
      }

      var card = event.target.closest('.tmux-session-card');
      if (!card) return;
      selectedSessionName = card.dataset.session;
      renderSessionList();
    });

    panelEl.addEventListener('dblclick', function (event) {
      var card = event.target.closest('.tmux-session-card');
      if (!card) return;
      attachSession(card.dataset.session);
    });

    if (listen) {
      listen('tmux-sessions-changed', function (event) {
        var payload = event.payload || {};
        if (Array.isArray(payload.sessions)) {
          sessions = payload.sessions.slice();
          if (!selectedSessionName && sessions[0]) {
            selectedSessionName = sessions[0].name;
          }
          renderSessionList();
        }
      });

      listen('tmux-connected', function (event) {
        var payload = event.payload || {};
        connectedSessionName = payload.session || connectedSessionName;
        selectedSessionName = connectedSessionName || selectedSessionName;
        initialAutoAttachAttempted = true;
        renderSessionList();
        refreshSessions();
      });

      listen('tmux-disconnected', function () {
        connectedSessionName = null;
        renderSessionList();
      });
    }
  }

  function refreshSessions() {
    if (!invoke) return Promise.resolve();
    return invoke('tmux_list_sessions').then(function (result) {
      sessions = Array.isArray(result) ? result.slice() : [];
      if (sessions.length === 0 && !autoCreateInFlight) {
        autoCreateInFlight = true;
        return invoke('tmux_create_session', { name: null }).then(function () {
          return invoke('tmux_list_sessions');
        }).then(function (createdResult) {
          sessions = Array.isArray(createdResult) ? createdResult.slice() : [];
          if (!selectedSessionName && sessions[0]) {
            selectedSessionName = sessions[0].name;
          }
          renderSessionList();
          if (sessions[0] && sessions[0].name) {
            return attachSession(sessions[0].name).then(function () {
              return sessions;
            });
          }
          return sessions;
        }).finally(function () {
          autoCreateInFlight = false;
        });
      }
      if (selectedSessionName && !sessions.some(function (session) { return session.name === selectedSessionName; })) {
        selectedSessionName = sessions[0] ? sessions[0].name : null;
      }
      if (!selectedSessionName && sessions[0]) {
        selectedSessionName = sessions[0].name;
      }
      renderSessionList();
      if (connectedSessionName || initialAutoAttachAttempted || autoAttachInFlight) {
        return sessions;
      }
      var attachedSession = sessions.find(function (session) {
        return session && (session.attached === true || Number(session.attached) === 1);
      });
      var targetSession = attachedSession && attachedSession.name
        ? String(attachedSession.name)
        : (selectedSessionName ? String(selectedSessionName) : (sessions[0] && sessions[0].name ? String(sessions[0].name) : null));
      if (!targetSession) {
        return sessions;
      }
      initialAutoAttachAttempted = true;
      autoAttachInFlight = true;
      return attachSession(targetSession).catch(function (error) {
        console.warn('[tmux] initial auto-attach failed', error);
        initialAutoAttachAttempted = false;
      }).finally(function () {
        autoAttachInFlight = false;
      }).then(function () {
        return sessions;
      });
    }).catch(function (error) {
      console.error('[tmux-panel] refresh error:', error);
      if (window.toast && typeof window.toast.error === 'function') {
        window.toast.error('Failed to load tmux sessions');
      }
      sessions = [];
      renderSessionList();
      return [];
    });
  }

  function createSession() {
    var name = null;
    var beforeNames = new Set((sessions || []).map(function (session) { return String(session.name || ''); }));
    console.info('[tmux] create session requested', {
      name: name || null,
      at: Date.now(),
    });
    invoke('tmux_create_session', { name: name || null }).then(function () {
      console.info('[tmux] create session resolved', {
        name: name || null,
        at: Date.now(),
      });
      if (window.toast) window.toast.success('Tmux session created');
      return refreshSessions();
    }).then(function (updatedSessions) {
      var createdSessionName = null;
      if (name) {
        createdSessionName = name;
      } else if (Array.isArray(updatedSessions)) {
        var createdCandidates = updatedSessions.filter(function (session) {
          return !beforeNames.has(String(session.name || ''));
        });
        if (createdCandidates.length > 0) {
          createdCandidates.sort(function (a, b) {
            var createdA = Number(a && a.created) || 0;
            var createdB = Number(b && b.created) || 0;
            if (createdA !== createdB) return createdB - createdA;
            return (Number(b && b.id) || 0) - (Number(a && a.id) || 0);
          });
          createdSessionName = String(createdCandidates[0].name || '');
        }
      }
      if (!createdSessionName && Array.isArray(updatedSessions) && updatedSessions.length > 0) {
        createdSessionName = String(updatedSessions[0].name || '');
      }
      if (createdSessionName) {
        selectedSessionName = createdSessionName;
        renderSessionList();
        return attachSession(createdSessionName);
      }
      return null;
    }).catch(function (error) {
      console.error('[tmux] create session failed', {
        name: name || null,
        error: String(error),
        at: Date.now(),
      });
      if (window.toast) window.toast.error('Failed to create tmux session: ' + error);
    });
  }

  function attachSession(name) {
    if (!name) return Promise.resolve();
    var switchToken = {
      targetSession: name,
      startedAt: Date.now(),
      suppressDisconnectsUntil: Date.now() + 5000,
    };
    window.__conchTmuxSwitchState = switchToken;
    console.info('[tmux] attach requested', switchToken);
    return invoke('tmux_connect', { sessionName: name }).then(function () {
      connectedSessionName = name;
      selectedSessionName = name;
      initialAutoAttachAttempted = true;
      renderSessionList();
      if (typeof window.__conchTmuxForceSyncSession === 'function') {
        window.__conchTmuxForceSyncSession(name);
      }
      if (window.toast) window.toast.success('Attached to ' + name);
    }).catch(function (error) {
      if (window.__conchTmuxSwitchState === switchToken) {
        window.__conchTmuxSwitchState = null;
      }
      if (window.toast) window.toast.error('Failed to attach tmux session: ' + error);
      throw error;
    });
  }

  function renameSession(name) {
    promptRenameSession(name).then(function (nextName) {
      if (!nextName || nextName === name) return null;
      return invoke('tmux_rename_session', { oldName: name, newName: nextName }).then(function () {
        if (connectedSessionName === name) connectedSessionName = nextName;
        if (selectedSessionName === name) selectedSessionName = nextName;
        if (window.toast) window.toast.success('Renamed session to ' + nextName);
        return refreshSessions();
      });
    }).catch(function (error) {
      if (window.toast) window.toast.error('Failed to rename tmux session: ' + error);
    });
  }

  function killSession(name) {
    confirmKillSession(name).then(function (confirmed) {
      if (!confirmed) return;
      return invoke('tmux_kill_session', { name: name }).then(function () {
        if (connectedSessionName === name) connectedSessionName = null;
        if (selectedSessionName === name) selectedSessionName = null;
        if (window.toast) window.toast.success('Killed tmux session ' + name);
        return refreshSessions();
      });
    }).catch(function (error) {
      if (window.toast) window.toast.error('Failed to kill tmux session: ' + error);
    });
  }

  function escapeHtml(value) {
    return String(value || '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function escapeAttr(value) {
    return escapeHtml(value).replace(/`/g, '&#96;');
  }

  exports.tmuxPanel = {
    init: init,
    refreshSessions: refreshSessions,
    createSession: createSession,
    renameCurrentSession: function () {
      if (selectedSessionName) renameSession(selectedSessionName);
    },
    killSessionPrompt: function () {
      if (selectedSessionName) killSession(selectedSessionName);
    },
  };
})(window);
