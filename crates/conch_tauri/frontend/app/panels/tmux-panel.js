/**
 * Tmux Sessions tool window.
 *
 * Displays tmux sessions with attach/create/rename/kill actions.
 * Live-updated via tmux-sessions-changed events from the backend.
 */
(function (exports) {
  'use strict';

  let invoke = null;
  let listen = null;
  let panelEl = null;
  let sessions = [];
  let selectedSessionName = null;

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
    panelEl.innerHTML = '';

    var toolbar = document.createElement('div');
    toolbar.className = 'tmux-panel-toolbar';
    toolbar.innerHTML = [
      '<button class="tmux-btn" data-action="new" title="New Session">+ New</button>',
      '<button class="tmux-btn" data-action="attach" title="Attach">Attach</button>',
      '<button class="tmux-btn" data-action="refresh" title="Refresh">\u21BB</button>',
    ].join('');
    panelEl.appendChild(toolbar);

    var listEl = document.createElement('div');
    listEl.className = 'tmux-session-list';
    listEl.id = 'tmux-session-list';
    panelEl.appendChild(listEl);

    renderSessionList();
  }

  function renderSessionList() {
    var listEl = document.getElementById('tmux-session-list');
    if (!listEl) return;
    var esc = window.utils ? window.utils.esc : function (s) { return s; };
    var attr = window.utils ? window.utils.attr : function (s) { return s; };

    if (sessions.length === 0) {
      listEl.innerHTML = [
        '<div class="tmux-empty-state">',
        '  <p>No tmux sessions found.</p>',
        '  <p>Create one to get started.</p>',
        '  <button class="tmux-btn tmux-create-btn" data-action="new">Create Session</button>',
        '</div>',
      ].join('');
      return;
    }

    listEl.innerHTML = sessions
      .map(function (s) {
        var indicator = s.attached ? '\u25CF' : '\u25CB';
        var selected = s.name === selectedSessionName ? ' tmux-session-selected' : '';
        var attached = s.attached ? ' tmux-session-attached' : '';
        var winLabel = s.window_count === 1 ? '1 win' : s.window_count + ' wins';
        return [
          '<div class="tmux-session-row' + selected + attached + '" data-session="' + attr(s.name) + '">',
          '  <span class="tmux-session-indicator">' + indicator + '</span>',
          '  <span class="tmux-session-name">' + esc(s.name) + '</span>',
          '  <span class="tmux-session-wins">' + winLabel + '</span>',
          '</div>',
        ].join('');
      })
      .join('');
  }

  function bindEvents() {
    panelEl.addEventListener('click', function (e) {
      var btn = e.target.closest('[data-action]');
      if (btn) {
        var action = btn.dataset.action;
        if (action === 'new') createSession();
        else if (action === 'attach') attachSelected();
        else if (action === 'refresh') refreshSessions();
        return;
      }
      var row = e.target.closest('.tmux-session-row');
      if (row) {
        selectedSessionName = row.dataset.session;
        renderSessionList();
      }
    });

    panelEl.addEventListener('dblclick', function (e) {
      var row = e.target.closest('.tmux-session-row');
      if (row) {
        attachSession(row.dataset.session);
      }
    });

    panelEl.addEventListener('contextmenu', function (e) {
      var row = e.target.closest('.tmux-session-row');
      if (!row) return;
      e.preventDefault();
      selectedSessionName = row.dataset.session;
      renderSessionList();
      showContextMenu(e.clientX, e.clientY, row.dataset.session);
    });

    if (listen) {
      listen('tmux-sessions-changed', function (event) {
        var payload = event.payload || {};
        if (payload.sessions) {
          sessions = payload.sessions;
          renderSessionList();
        }
      });
    }
  }

  function refreshSessions() {
    if (!invoke) return;
    invoke('tmux_list_sessions').then(function (result) {
      if (Array.isArray(result)) {
        sessions = result;
        renderSessionList();
      }
    }).catch(function (err) {
      console.error('[tmux-panel] refresh error:', err);
    });
  }

  function createSession() {
    var name = prompt('Session name (leave empty for default):');
    if (name === null) return;
    invoke('tmux_create_session', { name: name || null }).then(function () {
      refreshSessions();
    }).catch(function (err) {
      if (window.toast) window.toast.error('Failed to create session: ' + err);
    });
  }

  function attachSelected() {
    if (selectedSessionName) {
      attachSession(selectedSessionName);
    }
  }

  function attachSession(name) {
    invoke('tmux_connect', { sessionName: name }).catch(function (err) {
      if (window.toast) window.toast.error('Failed to attach: ' + err);
    });
  }

  function renameSession(name) {
    var newName = prompt('New session name:', name);
    if (!newName || newName === name) return;
    invoke('tmux_rename_session', { oldName: name, newName: newName }).then(function () {
      refreshSessions();
    }).catch(function (err) {
      if (window.toast) window.toast.error('Failed to rename: ' + err);
    });
  }

  function killSession(name) {
    if (!confirm('Kill session "' + name + '"? This will close all its windows.')) return;
    invoke('tmux_kill_session', { name: name }).then(function () {
      if (selectedSessionName === name) selectedSessionName = null;
      refreshSessions();
    }).catch(function (err) {
      if (window.toast) window.toast.error('Failed to kill session: ' + err);
    });
  }

  function showContextMenu(x, y, sessionName) {
    var existing = document.getElementById('tmux-context-menu');
    if (existing) existing.remove();

    var menu = document.createElement('div');
    menu.id = 'tmux-context-menu';
    menu.className = 'tmux-context-menu';
    menu.style.left = x + 'px';
    menu.style.top = y + 'px';
    menu.innerHTML = [
      '<div class="tmux-ctx-item" data-ctx="attach">Attach</div>',
      '<div class="tmux-ctx-item" data-ctx="rename">Rename</div>',
      '<div class="tmux-ctx-item tmux-ctx-danger" data-ctx="kill">Kill</div>',
    ].join('');

    menu.addEventListener('click', function (e) {
      var item = e.target.closest('[data-ctx]');
      if (!item) return;
      menu.remove();
      var action = item.dataset.ctx;
      if (action === 'attach') attachSession(sessionName);
      else if (action === 'rename') renameSession(sessionName);
      else if (action === 'kill') killSession(sessionName);
    });

    document.body.appendChild(menu);

    var closeMenu = function () {
      menu.remove();
      document.removeEventListener('click', closeMenu);
    };
    setTimeout(function () {
      document.addEventListener('click', closeMenu);
    }, 0);
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
