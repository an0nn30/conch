// File Explorer Panel — dual-pane local + remote file browser.

(function (exports) {
  'use strict';

  let invoke = null;
  let panelEl = null;
  let panelWrapEl = null;
  let resizeHandleEl = null;
  let layoutService = null;
  const filesDataService = exports.termlabFilesFeatureDataService || {};
  const filesPaneStore = exports.termlabFilesPaneStore || {};
  const filesActions = exports.termlabFilesActions || {};
  const filesPaneView = exports.termlabFilesPaneView || {};
  const filesTransfers = exports.termlabFilesTransfers || {};
  // Captured at module-load time, same as the deps above — connect-auth.js's
  // <script> tag MUST precede this one in index.html, or this permanently
  // binds to the {} fallback (see connect-auth.js's header comment).
  const connectAuth = exports.termlabConnectAuth || {};
  let fitActiveTabFn = null;
  let getActiveTabFn = null;
  let transferController = null;
  let unsubscribeTransferRuntime = null;
  // Project mode: this window has a project, so the panel renders a single
  // lazy tree instead of the dual-pane local+SFTP explorer. Per-window and
  // not persisted in v1 — the header toggle switches views for this session
  // only, which is what keeps SFTP fully reachable from a project window.
  let projectRoot = null;
  let projectMode = false;
  let projectTreeHandle = null;
  let projectRootMissing = false;
  // The exact markup dual-pane always lays down, declared once so a toggle
  // round trip can never let the project-window and plain-window render
  // paths drift out of sync with each other (task-6 review, F6).
  const DUAL_PANE_MARKUP = `
      <div class="fp-pane-container">
        <div class="fp-pane" id="fp-local"></div>
        <div class="fp-pane-divider" id="fp-pane-divider"></div>
        <div class="fp-pane" id="fp-remote"></div>
      </div>
    `;
  const FILES_TRANSFER_OPTIONS = Object.freeze({
    origin: 'filesPanel',
    conflictPolicy: Object.freeze({ kind: 'ask' }),
  });

  // Navigation icons — PNG assets from icons/ directory
  // Branded, appearance-aware toolbar glyphs (tl-icon resolves the light or
  // dark variant per render; refreshAll() heals stamped imgs on theme flips).
  // Resolved per call, not baked at module load, so a theme change after
  // startup renders the correct variant on the next pane render.
  function toolbarIcon(name) {
    return window.tlIcon && typeof window.tlIcon.html === 'function'
      ? window.tlIcon.html(name, { size: 12 })
      : '';
  }

  function createPaneState(prefix, isLocal) {
    if (!filesPaneStore || typeof filesPaneStore.createPaneState !== 'function') {
      throw new Error('files-pane-store missing createPaneState');
    }
    return filesPaneStore.createPaneState(prefix, isLocal);
  }

  // Pane state
  const localPane = createPaneState('local', true);
  const remotePane = createPaneState('remote', false);
  let activeRemotePaneId = null;
  let localCwdPollTimer = null;
  let localCwdPollInFlight = false;
  let lastLocalCwdByPaneId = new Map();
  let remoteCwdPollTimer = null;
  let remoteCwdPollInFlight = false;
  let lastRemoteCwdByPaneId = new Map();

  // ---------------------------------------------------------------------------
  // Remote host dropdown + pinning
  //
  // Follow mode (default): the remote pane tracks whichever SSH tab is
  // focused, via onTabChanged below. Pinning breaks that link — the pane
  // stays bound to one session (tab-owned or a detached SFTP-only
  // connection) regardless of what tab the user switches to.
  //
  // pinnedSessionKey is null in follow mode, else the "{window_label}:
  // {pane_id}" key (remote_get_sessions' ActiveSession.key) of the session
  // the remote pane is pinned to. It is the flag onTabChanged checks to
  // suppress its own rebinding while pinned — see the guard just above its
  // existing `tab.type !== 'ssh'` gate, which stays untouched.
  // ---------------------------------------------------------------------------

  let pinnedSessionKey = null;
  // Set to the server-entry id currently mid-connect, else null. Distinct
  // from pinnedSessionKey: a busy connect has no session yet (that is what
  // it is waiting for), so it cannot be represented as a pin.
  let hostConnectBusyEntryId = null;
  let cachedServers = { folders: [], ungrouped: [], ssh_config: [] };
  let cachedSessions = [];

  const HOST_COMBO_FOLLOW_VALUE = '';
  const HOST_COMBO_SEPARATOR_VALUE = '__separator__';
  // Mirrors crates/termlab_tauri/src/remote/detached_commands.rs's
  // DETACHED_PANE_ID_BASE. Detached (panel-only) sessions mint pane ids at
  // and above this value so they can never collide with a real terminal
  // pane's id; the frontend has no way to import the Rust constant, so this
  // is a deliberate duplicate — keep it in sync if that base ever moves.
  const DETACHED_PANE_ID_BASE = 1_000_000;

  // A session key's pane-id tail, or null if it doesn't parse as one.
  // Window labels are not expected to contain ':', matching how the backend
  // itself splits the key (detached_pane_id_for_window in
  // detached_commands.rs strips an exact "{window_label}:" prefix).
  function paneIdFromSessionKey(key) {
    if (typeof key !== 'string') return null;
    const idx = key.lastIndexOf(':');
    if (idx < 0) return null;
    const n = Number(key.slice(idx + 1));
    return Number.isFinite(n) ? n : null;
  }

  function isDetachedSessionKey(key) {
    const paneId = paneIdFromSessionKey(key);
    return paneId != null && paneId >= DETACHED_PANE_ID_BASE;
  }

  // Flattens configured hosts (folders + ungrouped + ssh_config) into combo
  // options, folder entries prefixed "FolderName / label". Vault-linking a
  // password connect promotes an ssh-config host into a config-owned copy
  // that SHARES THE SAME ID (T2 review finding F5) — so a host must appear
  // once, deduped by id, with the config-owned copy (folders/ungrouped)
  // always preferred over its ssh_config source.
  function buildConfiguredHostOptions(servers) {
    const data = servers && typeof servers === 'object'
      ? servers
      : { folders: [], ungrouped: [], ssh_config: [] };
    const seenIds = new Set();
    const options = [];

    (Array.isArray(data.folders) ? data.folders : []).forEach((folder) => {
      (Array.isArray(folder.entries) ? folder.entries : []).forEach((entry) => {
        if (!entry || seenIds.has(entry.id)) return;
        seenIds.add(entry.id);
        options.push({ value: entry.id, label: `${folder.name} / ${entry.label}`, kind: 'host' });
      });
    });
    (Array.isArray(data.ungrouped) ? data.ungrouped : []).forEach((entry) => {
      if (!entry || seenIds.has(entry.id)) return;
      seenIds.add(entry.id);
      options.push({ value: entry.id, label: entry.label, kind: 'host' });
    });
    // ssh_config entries only surface here when nothing config-owned has
    // claimed their id yet — i.e. they have not been vault-linked/promoted.
    (Array.isArray(data.ssh_config) ? data.ssh_config : []).forEach((entry) => {
      if (!entry || seenIds.has(entry.id)) return;
      seenIds.add(entry.id);
      options.push({ value: entry.id, label: entry.label, kind: 'host' });
    });

    return options;
  }

  // Combo composition: "Follow active tab" -> live sessions -> separator ->
  // configured hosts.
  function buildHostComboOptions(sessions, servers) {
    const options = [
      { value: HOST_COMBO_FOLLOW_VALUE, label: 'Follow active tab', kind: 'follow' },
    ];
    (Array.isArray(sessions) ? sessions : []).forEach((session) => {
      if (!session) return;
      const label = filesDataService && typeof filesDataService.sessionHostLabel === 'function'
        ? filesDataService.sessionHostLabel(session, paneIdFromSessionKey(session.key))
        : String(session.key);
      options.push({ value: session.key, label, kind: 'session' });
    });
    options.push({
      value: HOST_COMBO_SEPARATOR_VALUE,
      label: '──────────',
      kind: 'separator',
      disabled: true,
    });
    return options.concat(buildConfiguredHostOptions(servers));
  }

  // A server entry, by id, across folders/ungrouped/ssh_config — used only
  // by refreshHostCombo's busy-clear scoping (L1) to recognize when a
  // still-busy connect's target session has appeared. Mirrors connect-
  // auth.js's own findServerEntry (duplicated rather than shared: that
  // module's helper is private, and this one only needs an id lookup).
  function findServerEntryById(servers, entryId) {
    const data = servers && typeof servers === 'object'
      ? servers
      : { folders: [], ungrouped: [], ssh_config: [] };
    const folders = Array.isArray(data.folders) ? data.folders : [];
    for (let i = 0; i < folders.length; i += 1) {
      const entries = Array.isArray(folders[i].entries) ? folders[i].entries : [];
      const hit = entries.find((e) => e && e.id === entryId);
      if (hit) return hit;
    }
    const ungrouped = Array.isArray(data.ungrouped) ? data.ungrouped : [];
    const ungroupedHit = ungrouped.find((e) => e && e.id === entryId);
    if (ungroupedHit) return ungroupedHit;
    const sshConfig = Array.isArray(data.ssh_config) ? data.ssh_config : [];
    return sshConfig.find((e) => e && e.id === entryId) || null;
  }

  // Whether `session` (an ActiveSession from remote_get_sessions — key,
  // host, user, port; no server_entry_id, that would be a Rust change and
  // is out of scope here) looks like the session a connect to `entry` (a
  // ServerEntry) would produce. Host+port is the strong signal; user is
  // checked too when the entry pins one. Good enough to recognize "the busy
  // connect landed" without false-clearing on an unrelated session.
  function sessionMatchesEntry(session, entry) {
    if (!session || !entry) return false;
    if (session.host !== entry.host) return false;
    if (session.port !== entry.port) return false;
    if (entry.user && session.user !== entry.user) return false;
    return true;
  }

  function computeHostComboState() {
    return {
      hostOptions: buildHostComboOptions(cachedSessions, cachedServers),
      hostComboValue: hostConnectBusyEntryId || pinnedSessionKey || HOST_COMBO_FOLLOW_VALUE,
      hostComboBusy: !!hostConnectBusyEntryId,
      showDisconnect: !hostConnectBusyEntryId && !!pinnedSessionKey && isDetachedSessionKey(pinnedSessionKey),
    };
  }

  // Pin the remote pane to a specific session (sessionKey), or pass null to
  // return to follow mode. This is filesPanel.pinRemotePane, the entry
  // point Task 4's connect dialogs call on a successful connect/pick — it
  // binds the pane directly, bypassing tabs entirely, and sets the flag
  // that suppresses onTabChanged's own rebinding while pinned.
  async function pinRemotePane(sessionKey) {
    if (!hasPanelDom()) return;

    if (sessionKey == null) {
      pinnedSessionKey = null;
      renderPane(remotePane, getPaneRoot('#fp-remote'));
      return;
    }

    pinnedSessionKey = sessionKey;
    const paneId = paneIdFromSessionKey(sessionKey);
    if (paneId == null) {
      remotePane.error = `Not a valid session key: ${sessionKey}`;
      renderPane(remotePane, getPaneRoot('#fp-remote'));
      return;
    }
    activeRemotePaneId = paneId;
    remotePane.error = null;
    remotePane.backStack = [];
    remotePane.forwardStack = [];
    renderPane(remotePane, getPaneRoot('#fp-remote'));

    try {
      // Directory seeding for a pinned pane goes through sftp_realpath('.')
      // directly (getRemoteRealPath) — NOT ssh_get_pane_cwd/
      // pollActiveRemotePaneCwd, which assume the bound pane is the
      // currently-focused TAB and lack the caller->parent resolver a
      // detached session's synthetic pane id would need (pre-existing gap,
      // ledgered — see task-3-brief.md).
      const path = filesDataService && typeof filesDataService.getRemoteRealPath === 'function'
        ? await filesDataService.getRemoteRealPath(invoke, paneId, '.')
        : await Promise.reject(new Error('Files data service unavailable: getRemoteRealPath'));
      remotePane.currentPath = path;
      remotePane.pathInput = path;
      await loadEntries(remotePane);
    } catch (e) {
      remotePane.error = String(e);
      renderPane(remotePane, getPaneRoot('#fp-remote'));
    }
  }

  // Picking a configured host from the combo: busy state -> sftp_connect_host
  // -> pin on success. Rust already guards duplicate connects (same
  // window+entry returns the existing session), so this needn't debounce
  // picks itself.
  async function connectToHost(entryId) {
    hostConnectBusyEntryId = entryId;
    remotePane.error = null;
    renderPane(remotePane, getPaneRoot('#fp-remote'));

    try {
      const session = filesDataService && typeof filesDataService.connectHost === 'function'
        ? await filesDataService.connectHost(invoke, entryId)
        : await Promise.reject(new Error('Files data service unavailable: connectHost'));
      hostConnectBusyEntryId = null;
      await pinRemotePane(session.sessionKey);
    } catch (err) {
      if (err && err.kind === 'connectInProgress') {
        // Not an error — a connect for this host is already in flight
        // (Rust's duplicate-connect guard). The busy state persists; the
        // in-flight connect's own completion fires remote-sessions-changed,
        // which refreshes the combo and clears it. Handled inline, here,
        // rather than by connectAuth.run below: run() has no access to
        // hostConnectBusyEntryId (a private closure var of this module) to
        // leave it alone, and this check already existed before Task 4 — see
        // connect-auth.js's run() doc comment for the full reasoning.
        renderPane(remotePane, getPaneRoot('#fp-remote'));
        return;
      }
      // Task 4: every other non-Ok variant (vaultLocked, needsPassword,
      // authFailed, unreachable, other) drives the auth dialog chain — vault
      // unlock and/or a host-password prompt, possibly retrying
      // sftp_connect_host/sftp_connect_host_with_password — rather than just
      // being described as a static string. connectAuth.run never rejects:
      // it resolves the ConnectedSession the chain eventually won, or null
      // if the user cancelled or the chain ended in an error it already
      // routed to onError below.
      const session = connectAuth && typeof connectAuth.run === 'function'
        ? await connectAuth.run(entryId, err, {
          invoke,
          data: filesDataService,
          onError: (message) => { remotePane.error = message; },
        })
        : null;
      hostConnectBusyEntryId = null;
      if (session) {
        await pinRemotePane(session.sessionKey);
      } else {
        renderPane(remotePane, getPaneRoot('#fp-remote'));
      }
    }
  }

  function onHostComboChange(value) {
    if (!value || value === HOST_COMBO_FOLLOW_VALUE) {
      pinRemotePane(null);
      return;
    }
    if (value === HOST_COMBO_SEPARATOR_VALUE) return;
    const session = cachedSessions.find((s) => s && s.key === value);
    if (session) {
      pinRemotePane(value);
      return;
    }
    connectToHost(value);
  }

  // Disconnect (⏏) — only rendered for a pinned DETACHED session (see
  // computeHostComboState's showDisconnect). Terminal-owned sessions have no
  // disconnect affordance here: they die with their tab.
  function onDisconnectPinnedSession() {
    if (!pinnedSessionKey) return;
    const key = pinnedSessionKey;
    const disconnectPromise = filesDataService && typeof filesDataService.disconnectSession === 'function'
      ? filesDataService.disconnectSession(invoke, key)
      : Promise.reject(new Error('Files data service unavailable: disconnectSession'));
    disconnectPromise
      .then(() => pinRemotePane(null))
      .catch((e) => {
        remotePane.error = String(e);
        renderPane(remotePane, getPaneRoot('#fp-remote'));
      });
  }

  // Refetch servers + sessions and rebuild the combo. Runs at init and on
  // every remote-sessions-changed event (connect/disconnect/window-cleanup
  // — see detached_commands.rs's emit_sessions_changed). The event is
  // app-wide, not scoped to this window or this connect, so it is not by
  // itself proof that THIS window's busy connect resolved (L1). connectToHost
  // already clears hostConnectBusyEntryId directly on its own resolution
  // paths (success at :223, chain-return — success or the chain giving up —
  // at :253); the only busy state left standing here is the
  // `connectInProgress` case, deliberately left set because a connect for
  // this same (window, entry) is already running elsewhere. That case is
  // cleared below ONLY once the busy entry's session actually shows up in
  // the refreshed list — an unrelated sessions-changed event (another
  // window's connect/disconnect, or this window's own event racing ahead of
  // this refresh) must not re-enable the combo out from under a connect
  // that is still in flight. A failed winner emits no event at all (Rust
  // only emits on success/disconnect/window-destroy), so a busy connect
  // left waiting on a failed racer never gets this clear — it stays busy
  // until the user's own next pick or another session change; see L2. A pin
  // whose session vanished (the host disconnected out from under it) drops
  // back to follow mode with an error note, unrelated to the busy handling
  // above.
  async function refreshHostCombo() {
    if (!hasPanelDom()) return;
    try {
      const [servers, sessions] = await Promise.all([
        filesDataService && typeof filesDataService.getServers === 'function'
          ? filesDataService.getServers(invoke)
          : Promise.resolve({ folders: [], ungrouped: [], ssh_config: [] }),
        filesDataService && typeof filesDataService.getSessions === 'function'
          ? filesDataService.getSessions(invoke)
          : Promise.resolve([]),
      ]);
      cachedServers = servers;
      cachedSessions = sessions;
    } catch (_) {
      // Keep the previous cache; the next event retries.
    }

    if (hostConnectBusyEntryId) {
      const busyEntry = findServerEntryById(cachedServers, hostConnectBusyEntryId);
      if (busyEntry && cachedSessions.some((s) => sessionMatchesEntry(s, busyEntry))) {
        hostConnectBusyEntryId = null;
      }
    }

    if (pinnedSessionKey && !cachedSessions.some((s) => s && s.key === pinnedSessionKey)) {
      pinnedSessionKey = null;
      activeRemotePaneId = null;
      remotePane.entries = [];
      remotePane.currentPath = '';
      remotePane.error = 'The connected host disconnected.';
    }

    renderPane(remotePane, getPaneRoot('#fp-remote'));
  }

  function applyFollowPathSetting(enabled) {
    if (!filesPaneStore || typeof filesPaneStore.applyFollowPathSetting !== 'function') {
      console.error('files-pane-store missing applyFollowPathSetting');
      return;
    }
    filesPaneStore.applyFollowPathSetting(localPane, remotePane, enabled);
  }

  function loadFollowPathSetting() {
    if (!invoke) return;
    if (!filesDataService || typeof filesDataService.getAllSettings !== 'function') {
      console.error('files-data-service missing getAllSettings');
      applyFollowPathSetting(true);
      return;
    }
    if (!filesPaneStore || typeof filesPaneStore.getFollowPathFromSettings !== 'function') {
      console.error('files-pane-store missing getFollowPathFromSettings');
      applyFollowPathSetting(true);
      return;
    }
    const loadSettings = filesDataService.getAllSettings(invoke);
    loadSettings
      .then((settings) => {
        const follow = filesPaneStore.getFollowPathFromSettings(settings);
        applyFollowPathSetting(follow);
      })
      .catch(() => {
        applyFollowPathSetting(true);
      });
  }

  function init(opts) {
    invoke = opts.invoke;
    panelEl = opts.panelEl;
    panelWrapEl = opts.panelWrapEl;
    resizeHandleEl = opts.resizeHandleEl;
    layoutService = opts.layoutService
      || (window.termlabServices && window.termlabServices.layoutService)
      || null;
    fitActiveTabFn = opts.fitActiveTab;
    getActiveTabFn = opts.getActiveTab;
    projectRoot = opts.projectRoot || null;
    projectMode = !!projectRoot;
    const transferRuntime = window.termlabTransferRuntime;
    const transferDialogs = window.termlabTransferDialogs;
    transferController = filesTransfers && typeof filesTransfers.createController === 'function'
      ? filesTransfers.createController({
        localPane,
        remotePane,
        toast: window.toast,
        transferRuntime,
        transferDialogs,
        loadEntries,
        renderTransferStatus: (pane) => {
          const selector = pane && pane.isLocal ? '#fp-local' : '#fp-remote';
          renderPane(pane, getPaneRoot(selector));
        },
      })
      : null;

    if (typeof unsubscribeTransferRuntime === 'function') unsubscribeTransferRuntime();
    unsubscribeTransferRuntime = null;
    if (transferRuntime && typeof transferRuntime.subscribe === 'function'
        && transferController
        && typeof transferController.handleTransferSnapshot === 'function') {
      unsubscribeTransferRuntime = transferRuntime.subscribe((snapshot) => {
        transferController.handleTransferSnapshot(snapshot);
      });
    }

    if (!panelEl) {
      console.warn('filesPanel.init called without a panel element');
      return;
    }

    renderPanelBody();

    // Listen for transfer progress
    if (opts.listen) {
      opts.listen('transfer-progress', handleTransferProgress);
      opts.listen('config-changed', () => {
        loadFollowPathSetting();
      });
      opts.listen('remote-sessions-changed', () => {
        refreshHostCombo();
      });
      // OS file drops onto the remote pane (Task 8). Tauri v2 delivers these
      // ONLY through the window's onDragDropEvent API — a plain
      // `listen('tauri://drag-*')` registers against a different event target
      // and never fires (verified live: drops were silently ignored). One
      // subscription fans out to the hover/leave/drop handlers via the
      // dispatcher; `enter`/`over` share the hover-highlight handling,
      // `leave` always clears, `drop` hit-tests and routes.
      if (typeof opts.onDragDropEvent === 'function' && window.termlabNativeDrop) {
        opts.onDragDropEvent((event) => {
          window.termlabNativeDrop.dispatchNativeDragDropEvent(event, {
            hover: handleNativeDragHover,
            leave: handleNativeDragLeave,
            drop: handleNativeDrop,
          });
        });
      }
    }

    // refreshHostCombo() is no longer called here directly — renderDualPane()
    // (invoked above via renderPanelBody(), and again on every toggle back
    // from the project tree) now owns it, so a non-project window still gets
    // exactly one initial call and a project window's dual-pane view never
    // renders with a stale/blank remote pane (task-6 review, F2).
    loadFollowPathSetting();
    startLocalCwdPolling();
    startRemoteCwdPolling();
  }

  // The panel has exactly two shapes and one switch between them. Every
  // dual-pane behaviour below this line is untouched: a non-project window
  // takes renderDualPane and nothing else in this file behaves differently.
  function renderPanelBody() {
    if (!panelEl) return;
    if (projectTreeHandle) {
      projectTreeHandle.destroy();
      projectTreeHandle = null;
    }
    panelEl.innerHTML = '';
    if (projectMode && projectRoot) renderProjectTree();
    else renderDualPane();
  }

  function renderDualPane() {
    if (projectRoot) {
      // A project window toggling back from the tree already has a header
      // appended below, so the pane markup has to be ADDED rather than
      // replace panelEl's contents wholesale — built as real DOM nodes,
      // sibling to the header, rather than an innerHTML template (which
      // would wipe it). .fp-pane-container stays a direct child of panelEl:
      // panels.css sizes it with flex: 1 against #files-panel's own flex
      // column, which an intervening wrapper div would break.
      const header = document.createElement('div');
      header.className = 'fp-project-header';
      const label = document.createElement('span');
      label.className = 'fp-project-header__name';
      label.textContent = 'Local + Remote';
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'tl-project-tree__button';
      toggle.textContent = 'Project';
      toggle.setAttribute('data-files-mode-toggle', 'project');
      toggle.setAttribute('aria-label', 'Switch to the project tree');
      toggle.addEventListener('click', () => setProjectMode(true));
      header.appendChild(label);
      header.appendChild(toggle);
      panelEl.appendChild(header);

      // Built through a detached wrapper rather than panelEl.innerHTML
      // directly — that would wipe the header just appended above. This
      // branch already requires `document` for the header, so a throwaway
      // element costs nothing extra, and it keeps the markup textually
      // identical to the else branch below (DUAL_PANE_MARKUP, declared once
      // at module scope) instead of a second hand-maintained copy that could
      // drift out of sync with it (task-6 review, F6).
      const wrapper = document.createElement('div');
      wrapper.innerHTML = DUAL_PANE_MARKUP;
      panelEl.appendChild(wrapper.firstElementChild);
    } else {
      // No project, no header, no risk of wiping anything — panelEl was just
      // cleared by renderPanelBody, so a single innerHTML template is the
      // simplest way to lay down the pane markup (also keeps this branch
      // working against the plain-object panelEl stubs the older, non-
      // project files-panel test harnesses use, which do not implement a
      // real DOM's element-construction API or a `document` global at all).
      panelEl.innerHTML = DUAL_PANE_MARKUP;
    }

    // Resizable splitter between the panes. Orientation is read per-drag from
    // the computed style because the zone CSS flips the container to a column
    // when this tool window docks in a sidebar.
    if (window.termlabFilesSplit) {
      const containerEl = panelEl.querySelector('.fp-pane-container');
      window.termlabFilesSplit.attach({
        container: containerEl,
        firstEl: panelEl.querySelector('#fp-local'),
        secondEl: panelEl.querySelector('#fp-remote'),
        dividerEl: panelEl.querySelector('#fp-pane-divider'),
        storage: window.localStorage,
        getOrientation: () => (
          window.getComputedStyle(containerEl).flexDirection === 'column' ? 'column' : 'row'
        ),
      });
    }

    // Start local pane at home
    const homePromise = filesDataService && typeof filesDataService.getHomeDir === 'function'
      ? filesDataService.getHomeDir(invoke)
      : Promise.reject(new Error('Files data service unavailable: getHomeDir'));
    homePromise.then((home) => {
      localPane.currentPath = home;
      localPane.pathInput = home;
      loadEntries(localPane);
    }).catch(() => {
      localPane.currentPath = '/';
      localPane.pathInput = '/';
      loadEntries(localPane);
    });

    // The remote half's counterpart to the home-dir bootstrap above. init()
    // used to call this once, itself, after the (then-unconditional) initial
    // render — fine when dual-pane was the only shape, but a project window
    // toggling back from the tree gets a brand-new #fp-remote element every
    // time renderDualPane runs, and nothing else ever repopulates it: the
    // combo/session cache is still warm from the FIRST render, so no fetch
    // re-fires and the remote pane is left permanently blank until some
    // unrelated event (a session connecting) happens to trigger a redraw
    // (task-6 review, F2). Calling it here, on every renderDualPane, is what
    // makes the toggle round trip actually show the SFTP side again.
    refreshHostCombo();
  }

  function renderProjectTree() {
    const header = document.createElement('div');
    header.className = 'fp-project-header';
    const label = document.createElement('span');
    label.className = 'fp-project-header__name';
    label.textContent = (window.termlabProjectMode && window.termlabProjectMode.name()) || projectRoot;
    label.title = projectRoot;
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'tl-project-tree__button';
    toggle.textContent = 'SFTP';
    toggle.setAttribute('data-files-mode-toggle', 'dual');
    toggle.setAttribute('aria-label', 'Switch to the local and remote file explorer');
    toggle.addEventListener('click', () => setProjectMode(false));
    header.appendChild(label);
    header.appendChild(toggle);
    panelEl.appendChild(header);

    if (!window.termlabProjectTree || typeof window.termlabProjectTree.create !== 'function') {
      console.error('files-panel: project-tree module is unavailable');
      return;
    }
    projectTreeHandle = window.termlabProjectTree.create({
      invoke,
      root: projectRoot,
      showHidden: false,
      onOpenFile: (filePath) => {
        Promise.resolve(openTreeFile(filePath)).catch((error) => {
          console.error('files-panel: could not open in editor', error);
          window.toast.error('Could Not Open File', String(error));
        });
      },
      onContextMenu: (event, node) => {
        if (!filesPaneView || typeof filesPaneView.showRowContextMenu !== 'function') return;
        filesPaneView.showRowContextMenu(event, buildTreeContextMenuItems(node));
      },
      onReopen: () => {
        Promise.resolve(invoke('project_pick_folder'))
          .then((picked) => (picked ? invoke('project_open', { path: picked }) : null))
          .catch((e) => window.toast.error('Cannot Open Folder', String(e)));
      },
      toastError: (title, body) => window.toast.error(title, body),
    });
    panelEl.appendChild(projectTreeHandle.element);
    projectTreeHandle.refreshAll();
    checkProjectRootPresence();

    // F13 (task-6 review): project-tree.js's own contextmenu listener lives
    // on the internal `list` element and only fires `onContextMenu` when the
    // right-click resolves to a row ([data-tree-path]) — a click on the
    // tree's background (an empty project, or the empty space below the
    // last row) bubbles past it untouched and reaches here instead, with the
    // OS/browser default context menu still live unless handled. Checking
    // for a row ancestor before acting is what keeps this from double-firing
    // a menu for an actual row click: project-tree.js's listener does not
    // call stopPropagation(), so every row right-click reaches this handler
    // too, and the `onRow` check is what makes it a no-op there.
    projectTreeHandle.element.addEventListener('contextmenu', (event) => {
      const onRow = event.target && typeof event.target.closest === 'function'
        && event.target.closest('[data-tree-path]');
      if (onRow) return;
      if (typeof event.preventDefault === 'function') event.preventDefault();
      if (!filesPaneView || typeof filesPaneView.showRowContextMenu !== 'function') return;
      filesPaneView.showRowContextMenu(event, buildRootContextMenuItems());
    });
  }

  // The same route the dual-pane explorer's local rows take: the editor
  // service owns the ownership protocol and the jump trail records the open.
  function openTreeFile(filePath) {
    const service = window.termlabEditorService;
    if (!service || typeof service.openLocalFile !== 'function') {
      window.toast.error('Could Not Open File', 'The editor is unavailable in this window.');
      return null;
    }
    return service.openLocalFile(filePath);
  }

  function setProjectMode(on) {
    if (!projectRoot) return;
    projectMode = on === true;
    renderPanelBody();
    if (typeof fitActiveTabFn === 'function') fitActiveTabFn();
  }

  function isProjectMode() {
    return projectMode;
  }

  function projectTree() {
    return projectTreeHandle;
  }

  // The root can vanish while the window is open. Open editor tabs are
  // untouched; only the tree changes state.
  function checkProjectRootPresence() {
    if (!projectRoot || !projectTreeHandle) return Promise.resolve();
    // filesDataService.statLocal, not a raw invoke('local_stat', ...) —
    // matches doNewFile's own stat call and every other local-fs read in
    // this file (task-6 review, F10).
    const statPromise = filesDataService && typeof filesDataService.statLocal === 'function'
      ? filesDataService.statLocal(invoke, projectRoot)
      : invoke('local_stat', { path: projectRoot });
    return Promise.resolve(statPromise)
      .then((entry) => {
        projectRootMissing = !(entry && entry.is_dir);
        projectTreeHandle.setMissing(projectRootMissing);
      })
      .catch(() => {
        projectRootMissing = true;
        projectTreeHandle.setMissing(true);
      });
  }

  function hasPanelDom() {
    return !!panelEl;
  }

  function getActivePaneIdForType(expectedType) {
    const activeTab = getActiveTabFn ? getActiveTabFn() : null;
    if (!activeTab || activeTab.type !== expectedType) return null;
    if (activeTab.paneId != null) return activeTab.paneId;
    if (activeTab.focusedPaneId != null) return activeTab.focusedPaneId;
    if (activeTab.id != null) return activeTab.id;
    return null;
  }

  function getPaneRoot(selector) {
    return panelEl ? panelEl.querySelector(selector) : null;
  }

  // ---------------------------------------------------------------------------
  // Panel visibility & resize (mirrors ssh-panel pattern)
  // ---------------------------------------------------------------------------

  function isHidden() {
    return !window.toolWindowManager.isVisible('file-explorer');
  }
  function showPanel() {
    window.toolWindowManager.activate('file-explorer');
  }
  function hidePanel() {
    window.toolWindowManager.deactivate('file-explorer');
  }
  function togglePanel() {
    window.toolWindowManager.toggle('file-explorer');
  }

  // ---------------------------------------------------------------------------
  // Remote pane — activate on SSH tab switch
  // ---------------------------------------------------------------------------

  async function onTabChanged(tab) {
    if (!hasPanelDom()) return;
    if (tab && tab.type === 'local') {
      const paneId = tab.paneId != null ? tab.paneId : tab.focusedPaneId;
      if (paneId != null) {
        pollActiveLocalPaneCwd(paneId);
      }
    }
    // Pinning bypasses tabs entirely: pinRemotePane binds the remote pane's
    // id directly, and this flag suppresses onTabChanged's own rebinding
    // below for as long as the pin holds. Unpinning (pinRemotePane(null))
    // just clears the flag — normal follow behavior resumes on the NEXT tab
    // event, not by this call retroactively re-running the gate below.
    if (pinnedSessionKey) return;
    if (!tab || tab.type !== 'ssh' || !tab.spawned) {
      activeRemotePaneId = null;
      remotePane.entries = [];
      remotePane.currentPath = '';
      remotePane.error = null;
      remotePane.loading = false;
      renderPane(remotePane, getPaneRoot('#fp-remote'));
      return;
    }
    // Accept either a pane object (with .paneId) or a tab object (with .id).
    const id = tab.paneId != null ? tab.paneId : tab.id;
    if (activeRemotePaneId === id) return;
    activeRemotePaneId = id;
    pollActiveRemotePaneCwd(id);

    try {
      const path = filesDataService && typeof filesDataService.getRemoteRealPath === 'function'
        ? await filesDataService.getRemoteRealPath(invoke, id, '.')
        : await Promise.reject(new Error('Files data service unavailable: getRemoteRealPath'));
      remotePane.currentPath = path;
      remotePane.pathInput = path;
      remotePane.backStack = [];
      remotePane.forwardStack = [];
      await loadEntries(remotePane);
    } catch (e) {
      remotePane.error = String(e);
      renderPane(remotePane, getPaneRoot('#fp-remote'));
    }
  }

  function startLocalCwdPolling() {
    if (localCwdPollTimer) clearInterval(localCwdPollTimer);
    localCwdPollTimer = setInterval(() => {
      const paneId = getActivePaneIdForType('local');
      if (paneId == null) return;
      pollActiveLocalPaneCwd(paneId);
    }, 600);
  }

  function startRemoteCwdPolling() {
    if (remoteCwdPollTimer) clearInterval(remoteCwdPollTimer);
    remoteCwdPollTimer = setInterval(() => {
      const paneId = getActivePaneIdForType('ssh');
      if (paneId == null) return;
      pollActiveRemotePaneCwd(paneId);
    }, 600);
  }

  function pollActiveLocalPaneCwd(paneId) {
    if (!invoke || localCwdPollInFlight || paneId == null) return;
    const activePaneId = getActivePaneIdForType('local');
    if (activePaneId !== paneId) return;

    localCwdPollInFlight = true;
    const localCwdPromise = filesDataService && typeof filesDataService.getLocalPaneCwd === 'function'
      ? filesDataService.getLocalPaneCwd(invoke, paneId)
      : Promise.reject(new Error('Files data service unavailable: getLocalPaneCwd'));
    localCwdPromise
      .then((path) => {
        if (!path) return;
        if (lastLocalCwdByPaneId.get(paneId) === path) return;
        lastLocalCwdByPaneId.set(paneId, path);
        if (localPane.followCwd && path !== localPane.currentPath) {
          navigate(localPane, path);
        }
      })
      .catch(() => {})
      .finally(() => {
        localCwdPollInFlight = false;
      });
  }

  function pollActiveRemotePaneCwd(paneId) {
    // Mirrors onTabChanged's pin gate (:480): while pinned, the remote pane
    // is bound directly to pinnedSessionKey and must never be re-navigated
    // by the focused TAB's cwd — this poll runs off getActivePaneIdForType
    // ('ssh'), which tracks the focused tab regardless of the pin, so
    // without this gate a pinned pane would get yanked onto the focused
    // tab's cwd as a path on the PINNED host (M1: wrong listing / error
    // loop). Bail before even invoking, same as onTabChanged's early return.
    if (pinnedSessionKey) return;
    if (!invoke || remoteCwdPollInFlight || paneId == null) return;
    const activePaneId = getActivePaneIdForType('ssh');
    if (activePaneId !== paneId) return;

    remoteCwdPollInFlight = true;
    console.info('[files-cwd] polling ssh pane cwd', paneId);
    const remoteCwdPromise = filesDataService && typeof filesDataService.getRemotePaneCwd === 'function'
      ? filesDataService.getRemotePaneCwd(invoke, paneId)
      : Promise.reject(new Error('Files data service unavailable: getRemotePaneCwd'));
    remoteCwdPromise
      .then((path) => {
        if (!path) {
          console.info('[files-cwd] ssh pane cwd empty', paneId);
          return;
        }
        console.info('[files-cwd] ssh pane cwd resolved', paneId, path);
        if (lastRemoteCwdByPaneId.get(paneId) === path) return;
        lastRemoteCwdByPaneId.set(paneId, path);
        if (remotePane.followCwd && path !== remotePane.currentPath) {
          navigate(remotePane, path);
        }
      })
      .catch((e) => {
        console.warn('Remote cwd poll failed for pane', paneId, e);
      })
      .finally(() => {
        remoteCwdPollInFlight = false;
      });
  }

  // ---------------------------------------------------------------------------
  // Data loading
  // ---------------------------------------------------------------------------

  async function loadEntries(pane) {
    if (!hasPanelDom()) return;
    const generation = (pane.loadGeneration || 0) + 1;
    pane.loadGeneration = generation;
    pane.error = null;
    pane.loading = true;
    const el = getPaneRoot(`#fp-${pane.prefix}`);
    renderPane(pane, el);

    try {
      let entries;
      const path = pane.currentPath;
      if (pane.isLocal) {
        entries = filesDataService && typeof filesDataService.listLocalDir === 'function'
          ? await filesDataService.listLocalDir(invoke, path)
          : await Promise.reject(new Error('Files data service unavailable: listLocalDir'));
      } else {
        const paneId = activeRemotePaneId;
        if (!paneId) {
          pane.entries = [];
          pane.loading = false;
          renderPane(pane, el);
          return;
        }
        entries = filesDataService && typeof filesDataService.listRemoteDir === 'function'
          ? await filesDataService.listRemoteDir(invoke, paneId, path)
          : await Promise.reject(new Error('Files data service unavailable: listRemoteDir'));
      }
      if (pane.loadGeneration !== generation) return;
      pane.entries = entries;
      sortEntries(pane);
    } catch (e) {
      if (pane.loadGeneration !== generation) return;
      pane.error = String(e);
      pane.entries = [];
    }
    pane.loading = false;
    renderPane(pane, el);
  }

  function sortEntries(pane) {
    if (!filesPaneStore || typeof filesPaneStore.sortEntries !== 'function') {
      console.error('files-pane-store missing sortEntries');
      return;
    }
    filesPaneStore.sortEntries(pane);
  }

  // ---------------------------------------------------------------------------
  // Navigation
  // ---------------------------------------------------------------------------

  function actionDeps() {
    return {
      loadEntries,
      getHomeDir: async () => (
        filesDataService && typeof filesDataService.getHomeDir === 'function'
          ? filesDataService.getHomeDir(invoke)
          : Promise.reject(new Error('Files data service unavailable: getHomeDir'))
      ),
      // openInEditor is async and nothing awaits it, so without this a rejection
      // after its first `await` — or a throw from the toast it uses to report
      // one — would be an unhandled rejection and the double-click would look
      // like it did nothing at all.
      onOpenFile: (pane, entry, path) => {
        Promise.resolve(openInEditor(pane, entry, path)).catch((error) => {
          console.error('files-panel: could not open in editor', error);
          if (window.toast && typeof window.toast.error === 'function') {
            window.toast.error('Could Not Open File', String(error && error.message ? error.message : error));
          }
        });
      },
    };
  }

  // ---------------------------------------------------------------------------
  // Open in editor (double-click / Enter on a file row)
  // ---------------------------------------------------------------------------

  // The window label never changes for the life of the window, so one lookup
  // is enough. A failure here is not fatal — it only costs the host's name.
  let windowLabelPromise = null;
  function currentWindowLabel() {
    if (!windowLabelPromise) {
      windowLabelPromise = (
        filesDataService && typeof filesDataService.getCurrentWindowLabel === 'function'
          ? filesDataService.getCurrentWindowLabel(invoke)
          : Promise.resolve(null)
      ).catch(() => null);
    }
    return windowLabelPromise;
  }

  // A stable, human-readable name for the host a remote pane is connected to.
  // The editor hashes it into the temp path, so it is what keeps the same
  // filename on two different hosts in two different tabs. Pane objects carry
  // no host identity of their own; remote_get_sessions keys its entries by
  // "{window_label}:{pane_id}", which is why both halves are needed.
  //
  // The formula itself is `filesDataService.sessionHostLabel` — shared with
  // features/editor/file-dialog.js's ⌘O chooser, which must produce the
  // byte-identical label for the same session or the same remote file opens
  // in two tabs depending on which surface it was opened from. This function
  // owns only the lookup (window label + session by key); it must not
  // re-implement the formula.
  async function remoteHostLabel(paneId) {
    const fallback = `pane-${paneId}`;
    if (!filesDataService
      || typeof filesDataService.getSessions !== 'function'
      || typeof filesDataService.sessionHostLabel !== 'function') return fallback;
    try {
      const [label, sessions] = await Promise.all([
        currentWindowLabel(),
        filesDataService.getSessions(invoke),
      ]);
      if (!label) return fallback;
      const key = `${label}:${paneId}`;
      const session = (sessions || []).find((s) => s && s.key === key);
      return filesDataService.sessionHostLabel(session, paneId);
    } catch (_) {
      return fallback;
    }
  }

  // Lazily created instance of the parent-state event bridge's REVERSE half
  // (app/core/panel-host-bridge.js's publishAction) — the escape hatch this
  // window uses to hand a file open to its PARENT's editor when this window
  // has none of its own. Created once, on first need (`invoke` must already
  // be set, i.e. after init()), same laziness as windowLabelPromise above.
  let hostActionBridge;
  function getHostActionBridge() {
    if (hostActionBridge !== undefined) return hostActionBridge;
    hostActionBridge = (window.termlabPanelHostBridge && typeof window.termlabPanelHostBridge.create === 'function')
      ? window.termlabPanelHostBridge.create({ invoke })
      : null;
    return hostActionBridge;
  }

  async function openInEditor(pane, entry, path) {
    // A popped-out panel host has no editor of its own: editor-service.js's
    // createEditorTab throws unless manager-compose-runtime.js has run in
    // THIS window, and that module only ever composes for a real main
    // window (a host boot skips it entirely — app/panel-host-runtime.js's
    // module doc). `__termlabCreateEditorTab` is exactly the global
    // createEditorTab itself gates on, so it is the true test of whether
    // opening locally would even work. `window.termlabEditorService` is NOT
    // that signal — editor-service.js's <script> tag loads in every window
    // index.html serves, host or not, so the object it publishes always
    // exists even where none of its methods can succeed.
    if (typeof window.__termlabCreateEditorTab !== 'function') {
      const bridge = getHostActionBridge();
      if (bridge && typeof bridge.publishAction === 'function') {
        if (pane.isLocal) {
          bridge.publishAction('open-in-editor', { kind: 'local', path });
          return;
        }
        if (!activeRemotePaneId) return;
        // Read the pane id once: the user can switch tabs while the host
        // label is being resolved, and the download has to go to the
        // session this row was actually listed from.
        const paneId = activeRemotePaneId;
        const hostLabel = await remoteHostLabel(paneId);
        bridge.publishAction('open-in-editor', {
          kind: 'remote',
          paneId,
          remotePath: path,
          hostLabel,
          size: entry.size,
        });
        return;
      }
      // No bridge either (script graph missing it, or invoke never set) —
      // fall through to the ordinary "no editor" toast below.
    }

    const editor = window.termlabEditorService;
    if (!editor) {
      window.toast.error('Editor Unavailable', 'The editor service is not loaded.');
      return;
    }
    if (pane.isLocal) {
      editor.openLocalFile(path);
      return;
    }
    if (!activeRemotePaneId) return;
    // Read the pane id once: the user can switch tabs while the host label is
    // being resolved, and the download has to go to the session this row was
    // actually listed from.
    const paneId = activeRemotePaneId;
    const hostLabel = await remoteHostLabel(paneId);
    editor.openRemoteFile({
      paneId,
      remotePath: path,
      hostLabel,
      size: entry.size,
    });
  }

  function navigate(pane, path) {
    if (!filesActions || typeof filesActions.navigate !== 'function') {
      console.error('files-actions missing navigate');
      return;
    }
    filesActions.navigate(pane, path, actionDeps());
  }

  function goBack(pane) {
    if (!filesActions || typeof filesActions.goBack !== 'function') {
      console.error('files-actions missing goBack');
      return;
    }
    filesActions.goBack(pane, actionDeps());
  }

  function goForward(pane) {
    if (!filesActions || typeof filesActions.goForward !== 'function') {
      console.error('files-actions missing goForward');
      return;
    }
    filesActions.goForward(pane, actionDeps());
  }

  async function goHome(pane) {
    if (!filesActions || typeof filesActions.goHome !== 'function') {
      console.error('files-actions missing goHome');
      return;
    }
    await filesActions.goHome(pane, actionDeps());
  }

  function activateEntry(pane, entry) {
    if (!filesActions || typeof filesActions.activateEntry !== 'function') {
      console.error('files-actions missing activateEntry');
      return;
    }
    filesActions.activateEntry(pane, entry, actionDeps());
  }

  // ---------------------------------------------------------------------------
  // Rendering
  // ---------------------------------------------------------------------------

  function renderPane(pane, el) {
    if (!el) return;
    if (!filesPaneView || typeof filesPaneView.renderPane !== 'function') {
      console.error('files-pane-view missing renderPane');
      el.innerHTML = '<div class="fp-error">Files pane view module unavailable.</div>';
      return;
    }
    const comboState = computeHostComboState();
    filesPaneView.renderPane(pane, el, {
      activeRemotePaneId,
      iconBack: toolbarIcon('back'),
      iconForward: toolbarIcon('forward'),
      iconRefresh: toolbarIcon('refresh'),
      iconMore: toolbarIcon('moreVertical'),
      fileIcons: window.fileIcons,
      sortArrow,
      extOf,
      formatSize,
      formatDate,
      esc,
      attr,
      onActivateEntry: (entry) => activateEntry(pane, entry),
      onSelectEntry: (name) => { pane._selectedName = name; },
      onBack: () => goBack(pane),
      onForward: () => goForward(pane),
      onHome: () => goHome(pane),
      onRefresh: () => loadEntries(pane),
      onToggleHidden: () => { pane.showHidden = !pane.showHidden; renderPane(pane, el); },
      onNavigate: (path) => navigate(pane, path),
      onSort: (col) => {
        if (pane.sortColumn === col) pane.sortAscending = !pane.sortAscending;
        else { pane.sortColumn = col; pane.sortAscending = true; }
        sortEntries(pane);
        renderPane(pane, el);
      },
      onOpenColumnMenu: (event) => showColumnMenu(event, pane, el),
      onOpenRowMenu: (event, entry) => showRowContextMenu(event, pane, entry),
      joinPath,
      onDropEntries: (payload) => onDropEntries(payload),
      // Pane-to-pane drags complete over the NATIVE channel on platforms
      // where Tauri's drag-drop interception swallows DOM dragover/drop
      // (macOS does; proven live). dragstart records the in-flight payload;
      // the native drop with empty paths consumes it (see handleNativeDrop).
      onDragStart: (payload) => {
        if (internalDragClearTimer) { clearTimeout(internalDragClearTimer); internalDragClearTimer = null; }
        inFlightInternalDrag = payload || null;
      },
      // DOM dragend fires BEFORE wry delivers the native drop event (~30ms
      // gap, proven live), so the record must outlive dragend briefly: clear
      // highlights now, clear the record after a grace period long enough
      // for the trailing native drop to consume it first. A cancelled drag
      // (Escape / dropped outside the window) produces no native drop, and
      // the timer disposes of the record then.
      onDragEnd: () => {
        clearDropTargets();
        if (internalDragClearTimer) clearTimeout(internalDragClearTimer);
        internalDragClearTimer = setTimeout(() => {
          internalDragClearTimer = null;
          inFlightInternalDrag = null;
        }, 500);
      },
      onTransferAttention: (transferId, invoker) => {
        const handled = transferController
          && typeof transferController.handleTransferAttention === 'function'
          && transferController.handleTransferAttention(transferId, invoker);
        if (!handled
            && window.toolWindowManager
            && typeof window.toolWindowManager.activate === 'function') {
          window.toolWindowManager.activate('transfer-center');
        }
      },
      hostOptions: comboState.hostOptions,
      hostComboValue: comboState.hostComboValue,
      hostComboBusy: comboState.hostComboBusy,
      showDisconnect: comboState.showDisconnect,
      onHostComboChange: (value) => onHostComboChange(value),
      onDisconnect: () => onDisconnectPinnedSession(),
      // tlCombo.attach must re-run after EVERY toolbar rebuild — pane-view's
      // renderPane rebuilds the toolbar via innerHTML on every call
      // (pane-view.js:28), which discards whatever <select>/button the
      // previous render attached. This callback is that re-attach: invoked
      // with the fresh <select> pane-view just built, every single render.
      onComboMount: (selectEl) => {
        if (window.tlCombo && typeof window.tlCombo.attach === 'function') {
          window.tlCombo.attach(selectEl);
        }
      },
    });
  }

  function sortArrow(pane, col) {
    if (pane.sortColumn !== col) return '';
    return pane.sortAscending ? ' \u25B4' : ' \u25BE';
  }

  function showColumnMenu(e, pane, el) {
    if (!filesPaneView || typeof filesPaneView.showColumnMenu !== 'function') {
      console.error('files-pane-view missing showColumnMenu');
      return;
    }
    filesPaneView.showColumnMenu(e, pane, {
      onToggleColumn: (key) => {
        pane[key] = !pane[key];
        renderPane(pane, el);
      },
    });
  }

  // ---------------------------------------------------------------------------
  // Transfers — reached from the row context menu (see buildRowContextMenuItems).
  // Local pane rows offer upload actions; remote pane rows offer download
  // actions, mirroring the reference app's per-pane transfer direction.
  // ---------------------------------------------------------------------------

  function joinPath(base, name) {
    const trimmed = String(base || '').replace(/\/+$/, '');
    return (trimmed || '') + '/' + name;
  }

  async function submitTransfer(pane, fileName, direction, start) {
    const provisional = { status: 'preparing', direction, provisional: true };
    pane.transferStatus[fileName] = provisional;
    renderPane(pane, getPaneRoot(`#fp-${pane.prefix}`));
    try {
      const transferId = await start();
      if (pane.transferStatus[fileName] === provisional) {
        pane.transferStatus[fileName] = { status: 'preparing', direction, transferId };
        renderPane(pane, getPaneRoot(`#fp-${pane.prefix}`));
      }
      return transferId;
    } catch (error) {
      if (pane.transferStatus[fileName] === provisional) {
        delete pane.transferStatus[fileName];
        renderPane(pane, getPaneRoot(`#fp-${pane.prefix}`));
      }
      throw error;
    }
  }

  async function doDownload(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory download not yet supported.'); return; }

    const remotePath = joinPath(remotePane.currentPath, entry.name);
    const localPath = joinPath(localPane.currentPath, entry.name);

    try {
      if (!filesDataService || typeof filesDataService.transferDownload !== 'function') {
        throw new Error('Files data service unavailable: transferDownload');
      }
      await submitTransfer(localPane, entry.name, 'download', () => (
        filesDataService.transferDownload(
          invoke,
          activeRemotePaneId,
          remotePath,
          localPath,
          FILES_TRANSFER_OPTIONS,
        )
      ));
    } catch (e) {
      window.toast.error('Download Failed', String(e));
    }
  }

  async function doUpload(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory upload not yet supported.'); return; }

    const localPath = joinPath(localPane.currentPath, entry.name);
    const remotePath = joinPath(remotePane.currentPath, entry.name);

    try {
      if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
        throw new Error('Files data service unavailable: transferUpload');
      }
      await submitTransfer(remotePane, entry.name, 'upload', () => (
        filesDataService.transferUpload(
          invoke,
          activeRemotePaneId,
          localPath,
          remotePath,
          FILES_TRANSFER_OPTIONS,
        )
      ));
    } catch (e) {
      window.toast.error('Upload Failed', String(e));
    }
  }

  async function doUploadToPath(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory upload not yet supported.'); return; }

    const localPath = joinPath(localPane.currentPath, entry.name);
    showTextPromptDialog({
      title: 'Upload to Path',
      label: 'Remote destination path',
      initialValue: joinPath(remotePane.currentPath, entry.name),
      confirmLabel: 'Upload',
      onConfirm: async (remotePath) => {
        try {
          if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
            throw new Error('Files data service unavailable: transferUpload');
          }
          await submitTransfer(remotePane, entry.name, 'upload', () => (
            filesDataService.transferUpload(
              invoke,
              activeRemotePaneId,
              localPath,
              remotePath,
              FILES_TRANSFER_OPTIONS,
            )
          ));
        } catch (e) {
          window.toast.error('Upload Failed', String(e));
        }
      },
    });
  }

  // Whole-folder transfers hand the tree to the backend's recursive-expansion
  // command (`transfer_enqueue_recursive`) instead of walking it client-side.
  // `destPath` is the opposite pane's current directory as-is — never
  // joined with the folder's own name — because the backend appends that
  // basename itself (see features/files/data-service.js's transferRecursive).
  // Every folder transfer this panel starts goes through here so the batch
  // it creates is registered with the transfer controller. A folder with no
  // files in it produces a batch with no member jobs, and every other
  // completion notice is aggregated from member jobs — without this
  // registration that transfer would finish in total silence (see
  // features/files/transfers.js's watchFolderBatch).
  async function startFolderTransfer(paneId, direction, sourcePath, destPath) {
    if (!filesDataService || typeof filesDataService.transferRecursive !== 'function') {
      throw new Error('Files data service unavailable: transferRecursive');
    }
    const batchId = await filesDataService.transferRecursive(invoke, paneId, direction, sourcePath, destPath);
    if (transferController && typeof transferController.watchFolderBatch === 'function') {
      transferController.watchFolderBatch(batchId);
    }
    return batchId;
  }

  async function doUploadFolder(entry) {
    if (!entry || !activeRemotePaneId) return;
    const sourcePath = joinPath(localPane.currentPath, entry.name);
    const destPath = remotePane.currentPath;
    try {
      await startFolderTransfer(activeRemotePaneId, 'upload', sourcePath, destPath);
      window.toast.info('Folder transfer started', entry.name);
    } catch (e) {
      window.toast.error('Upload Failed', String(e));
    }
  }

  async function doDownloadFolder(entry) {
    if (!entry || !activeRemotePaneId) return;
    const sourcePath = joinPath(remotePane.currentPath, entry.name);
    const destPath = localPane.currentPath;
    try {
      await startFolderTransfer(activeRemotePaneId, 'download', sourcePath, destPath);
      window.toast.info('Folder transfer started', entry.name);
    } catch (e) {
      window.toast.error('Download Failed', String(e));
    }
  }

  // Source-side dragend safety net (Task 7 review finding). pane-view.js's
  // rows call this on every dragend regardless of outcome — a cancelled
  // drag (Escape mid-drag, dropped outside any target) fires dragend on the
  // source row without necessarily firing dragleave on whichever pane's
  // highlight is lit, and pane-view.js has no handle to the SIBLING pane's
  // root to clear it itself. Clearing both unconditionally is simplest and
  // idempotent — at most one is ever actually lit.
  // The row payload a DOM dragstart recorded, consumed by the native-channel
  // drop when the platform intercepts DOM drag events (macOS). Consumed by
  // the native drop, or disposed by the grace-period timer dragend arms
  // (dragend precedes the native drop — see onDragEnd).
  let inFlightInternalDrag = null;
  let internalDragClearTimer = null;

  // Both panes' logical rects + current paths, for internal-drop hit-testing.
  function internalDropPanes() {
    const localRoot = getPaneRoot('#fp-local');
    const remoteRoot = getPaneRoot('#fp-remote');
    return {
      local: localRoot && typeof localRoot.getBoundingClientRect === 'function'
        ? { rect: localRoot.getBoundingClientRect(), currentPath: localPane.currentPath, root: localRoot }
        : null,
      remote: remoteRoot && typeof remoteRoot.getBoundingClientRect === 'function'
        ? { rect: remoteRoot.getBoundingClientRect(), currentPath: remotePane.currentPath, root: remoteRoot }
        : null,
    };
  }

  function clearDropTargets() {
    const localEl = getPaneRoot('#fp-local');
    if (localEl && localEl.classList) localEl.classList.remove('is-drop-target');
    const remoteEl = getPaneRoot('#fp-remote');
    if (remoteEl && remoteEl.classList) remoteEl.classList.remove('is-drop-target');
  }

  function basenameOfPath(p) {
    const trimmed = String(p || '').replace(/\/+$/, '');
    const idx = trimmed.lastIndexOf('/');
    return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
  }

  // Drag-and-drop between panes (Task 7) — the drop side of pane-view.js's
  // dragstart/dragover/drop wiring. `source` is the parsed payload
  // ({ paneKind, paneId, path, isDir }); `targetPaneKind`/`targetPath`
  // describe the pane the drop landed on. Routing mirrors the row-menu
  // transfer actions above exactly (doUpload/doDownload/doUploadFolder/
  // doDownloadFolder) — same guard, same call shapes, same destination-join
  // rule (recursive gets the container as-is; single-file pre-joins the
  // name) — just reached from a drop instead of a menu click. Same-kind
  // drops never reach here: pane-view.js's drop handler only calls
  // onDropEntries when the payload's paneKind differs from the target pane.
  async function onDropEntries({ source, targetPaneKind, targetPath } = {}) {
    if (!source) return;
    const direction = targetPaneKind === 'remote' ? 'upload' : 'download';
    if (!activeRemotePaneId) {
      window.toast.warn('Not Connected', direction === 'upload'
        ? 'Connect to an SSH session to upload files.'
        : 'Connect to an SSH session to download files.');
      return;
    }

    const name = basenameOfPath(source.path);
    try {
      if (source.isDir) {
        await startFolderTransfer(activeRemotePaneId, direction, source.path, targetPath);
        window.toast.info('Folder transfer started', name);
        return;
      }

      const destPath = joinPath(targetPath, name);
      if (direction === 'upload') {
        if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
          throw new Error('Files data service unavailable: transferUpload');
        }
        await submitTransfer(remotePane, name, 'upload', () => (
          filesDataService.transferUpload(invoke, activeRemotePaneId, source.path, destPath, FILES_TRANSFER_OPTIONS)
        ));
      } else {
        if (!filesDataService || typeof filesDataService.transferDownload !== 'function') {
          throw new Error('Files data service unavailable: transferDownload');
        }
        await submitTransfer(localPane, name, 'download', () => (
          filesDataService.transferDownload(invoke, activeRemotePaneId, source.path, destPath, FILES_TRANSFER_OPTIONS)
        ));
      }
    } catch (e) {
      window.toast.error(direction === 'upload' ? 'Upload Failed' : 'Download Failed', String(e));
    }
  }

  // ---------------------------------------------------------------------------
  // OS file drops onto the remote pane (Task 8) — features/files/native-drop.js
  // owns the pure hit-test (resolveNativeDrop) and per-path routing
  // (routeNativeDropPaths); this section is just the DOM/event wiring: pull
  // the event payload, scale its position, hand off, and reflect the result
  // as either the is-drop-target highlight or the drop routing/toast.
  // ---------------------------------------------------------------------------

  // Tauri v2 drag-drop positions are reported in PHYSICAL pixels, but
  // getBoundingClientRect() (what the hit-test compares against) is in
  // LOGICAL/CSS pixels — on a retina display (devicePixelRatio 2) a raw
  // physical position would land roughly twice as far right/down as it
  // should, so every position is divided by devicePixelRatio before it ever
  // reaches resolveNativeDrop. The scaling itself lives in
  // features/files/native-drop.js (loaded before this file — see
  // index.html) so core/dragdrop-runtime.js's terminal-drop hit-test (the
  // Task 8 fix-round collision fix) shares the exact same math rather than
  // keeping its own copy.
  function scaleNativeDropPosition(position) {
    const nativeDrop = window.termlabNativeDrop;
    return nativeDrop && typeof nativeDrop.scaleNativeDropPosition === 'function'
      ? nativeDrop.scaleNativeDropPosition(position)
      : null;
  }

  function nativeDropDeps() {
    return {
      statPath: async (path) => {
        if (!filesDataService || typeof filesDataService.statLocal !== 'function') {
          throw new Error('Files data service unavailable: statLocal');
        }
        const entry = await filesDataService.statLocal(invoke, path);
        return { isDir: !!(entry && entry.is_dir) };
      },
      transferRecursive: async (paneId, sourcePath, destPath) => (
        startFolderTransfer(paneId, 'upload', sourcePath, destPath)
      ),
      // Mirrors doUpload/onDropEntries exactly: routed through submitTransfer
      // so the remote pane shows the same "preparing" transfer-status row a
      // menu-driven or intra-app-dragged upload would.
      transferUpload: async (paneId, sourcePath, destPath) => {
        if (!filesDataService || typeof filesDataService.transferUpload !== 'function') {
          throw new Error('Files data service unavailable: transferUpload');
        }
        const name = basenameOfPath(sourcePath);
        return submitTransfer(remotePane, name, 'upload', () => (
          filesDataService.transferUpload(invoke, paneId, sourcePath, destPath, FILES_TRANSFER_OPTIONS)
        ));
      },
      targetPaneId: activeRemotePaneId,
      targetPath: remotePane.currentPath,
      toast: window.toast,
    };
  }

  // Shared by drag-enter and drag-over: both just update the hover highlight,
  // never route anything (only an actual drop does).
  function handleNativeDragHover(event) {
    // Internal (pane-to-pane) drag in flight: highlight the OPPOSITE pane
    // when the native position hits it; external logic below never runs.
    if (inFlightInternalDrag) {
      const nativeDropApi = window.termlabNativeDrop;
      const scaled = scaleNativeDropPosition(event && event.payload && event.payload.position);
      const panes = internalDropPanes();
      const verdict = nativeDropApi && typeof nativeDropApi.resolveInternalNativeDrop === 'function'
        ? nativeDropApi.resolveInternalNativeDrop(scaled, panes, inFlightInternalDrag)
        : null;
      clearDropTargets();
      if (verdict && panes[verdict.targetPaneKind] && panes[verdict.targetPaneKind].root.classList) {
        panes[verdict.targetPaneKind].root.classList.add('is-drop-target');
      }
      return;
    }
    const remoteRoot = getPaneRoot('#fp-remote');
    if (!remoteRoot || !remoteRoot.classList) return;
    const payload = event && event.payload;
    const position = scaleNativeDropPosition(payload && payload.position);
    const rect = remoteRoot.getBoundingClientRect();
    const nativeDrop = window.termlabNativeDrop;
    const result = nativeDrop && typeof nativeDrop.resolveNativeDrop === 'function'
      ? nativeDrop.resolveNativeDrop(position, rect, !!activeRemotePaneId)
      : 'ignore';
    if (result === 'accept') {
      remoteRoot.classList.add('is-drop-target');
    } else {
      remoteRoot.classList.remove('is-drop-target');
    }
  }

  function handleNativeDragLeave() {
    // A native leave during an internal drag means the cursor left the
    // window; the highlight clears but the in-flight record survives (the
    // drag may re-enter) — dragend is the authoritative cleanup.
    clearDropTargets();
  }

  async function handleNativeDrop(event) {
    // Internal (pane-to-pane) drop: the native channel delivers it with
    // EMPTY paths while a DOM dragstart's payload is in flight. Route it to
    // the opposite pane through the same onDropEntries path the (macOS-
    // intercepted) DOM drop would have used.
    if (inFlightInternalDrag) {
      const internalSource = inFlightInternalDrag;
      inFlightInternalDrag = null;
      const externalPaths = event && event.payload && Array.isArray(event.payload.paths)
        ? event.payload.paths
        : [];
      if (externalPaths.length === 0) {
        const nativeDropApi = window.termlabNativeDrop;
        const scaled = scaleNativeDropPosition(event && event.payload && event.payload.position);
        const panes = internalDropPanes();
        const verdict = nativeDropApi && typeof nativeDropApi.resolveInternalNativeDrop === 'function'
          ? nativeDropApi.resolveInternalNativeDrop(scaled, panes, internalSource)
          : null;
        clearDropTargets();
        if (verdict) {
          await onDropEntries({
            source: internalSource,
            targetPaneKind: verdict.targetPaneKind,
            targetPath: verdict.targetPath,
          });
        }
        return;
      }
      // Paths present while an internal drag was recorded: stale record —
      // fall through to the external routing below.
    }
    const remoteRoot = getPaneRoot('#fp-remote');
    if (!remoteRoot) return;
    if (remoteRoot.classList) remoteRoot.classList.remove('is-drop-target');

    const payload = event && event.payload;
    const position = scaleNativeDropPosition(payload && payload.position);
    const rect = remoteRoot.getBoundingClientRect();
    const nativeDrop = window.termlabNativeDrop;
    const result = nativeDrop && typeof nativeDrop.resolveNativeDrop === 'function'
      ? nativeDrop.resolveNativeDrop(position, rect, !!activeRemotePaneId)
      : 'ignore';

    if (result === 'ignore') return; // drop missed the remote pane — silent, per spec
    if (result === 'no-session') {
      window.toast.warn('Not Connected', 'Connect to an SSH session to upload files.');
      return;
    }

    const paths = payload && Array.isArray(payload.paths) ? payload.paths : [];
    if (!paths.length) return;
    if (!nativeDrop || typeof nativeDrop.routeNativeDropPaths !== 'function') {
      window.toast.error('Upload Failed', 'Native drop module unavailable.');
      return;
    }
    await nativeDrop.routeNativeDropPaths(paths, nativeDropDeps());
  }

  async function doDownloadToPath(entry) {
    if (!entry || !activeRemotePaneId) return;
    if (entry.is_dir) { window.toast.warn('Not Supported', 'Directory download not yet supported.'); return; }

    const remotePath = joinPath(remotePane.currentPath, entry.name);
    showTextPromptDialog({
      title: 'Download to Path',
      label: 'Local destination path',
      initialValue: joinPath(localPane.currentPath, entry.name),
      confirmLabel: 'Download',
      onConfirm: async (localPath) => {
        try {
          if (!filesDataService || typeof filesDataService.transferDownload !== 'function') {
            throw new Error('Files data service unavailable: transferDownload');
          }
          await submitTransfer(localPane, entry.name, 'download', () => (
            filesDataService.transferDownload(
              invoke,
              activeRemotePaneId,
              remotePath,
              localPath,
              FILES_TRANSFER_OPTIONS,
            )
          ));
        } catch (e) {
          window.toast.error('Download Failed', String(e));
        }
      },
    });
  }

  // ---------------------------------------------------------------------------
  // Row actions — New Folder / Rename / Delete / Copy Path (row context menu)
  // ---------------------------------------------------------------------------

  // The tree's context menu offers New File, which the dual-pane explorer
  // never had. An empty write through the editor's own writer keeps one
  // definition of "create a text file" rather than adding a second command.
  //
  // editor_write_file() (write_text_file in editor_fs.rs) writes
  // unconditionally — it has no create-vs-overwrite distinction, so calling
  // it straight through on a name that already exists would silently empty
  // that file. local_stat rejects when nothing exists at the target path,
  // which is the only case creation should proceed in — resolving instead
  // means something is already there, so refuse rather than truncate it.
  //
  // F8 (task-6 review): this is a best-effort stat-then-write, not an atomic
  // guarantee — a file created at `target` in the window between the stat
  // resolving "nothing here" and the write landing would still be silently
  // overwritten (classic TOCTOU). A durable fix needs a Rust command backed
  // by `OpenOptions::new().create_new(true)` (atomically fails if the path
  // exists, no separate stat), which is out of scope for this pass; the
  // in-app race window here is narrow (this window's own UI, one user) and
  // this guard already closes the overwhelmingly common case (retyping an
  // existing name by mistake).
  function doNewFile(dirPath, afterCreate) {
    showTextPromptDialog({
      title: 'New File',
      label: 'Name',
      initialValue: '',
      confirmLabel: 'Create',
      onConfirm: (name) => {
        const target = joinPath(dirPath, name);
        const statPromise = filesDataService && typeof filesDataService.statLocal === 'function'
          ? filesDataService.statLocal(invoke, target)
          : invoke('local_stat', { path: target });
        Promise.resolve(statPromise).then(
          () => {
            window.toast.error('New File Failed', `"${name}" already exists.`);
          },
          () => Promise.resolve(invoke('editor_write_file', { path: target, contents: '' }))
            .then(() => {
              if (typeof afterCreate === 'function') afterCreate();
              // CONTROLLER RULING (task-6 review, F12): a file created inside
              // a directory that is not currently expanded would otherwise
              // exist on disk but never render — the tree only walks a
              // subdirectory's children once it is in the `expanded` set, and
              // `afterCreate`'s refresh() alone does not add it there.
              // project-tree.js's expand() only fetches a listing when one
              // isn't already cached (`!listings.has(dirPath)`), so calling
              // it here alongside afterCreate's refresh() never double-fetches
              // — whichever of the two actually owns the fetch in a given
              // state (collapsed-and-never-listed vs already-expanded) is the
              // one whose await does real work; the other is a cheap re-render.
              // Opening the new file straight into the editor is the second
              // half of the ruling — the IDE convention for "create a file"
              // is to land the user in it, not leave them to find and click
              // it themselves.
              if (projectTreeHandle) projectTreeHandle.expand(dirPath);
              Promise.resolve(openTreeFile(target)).catch((error) => {
                console.error('files-panel: could not open new file', error);
                window.toast.error('Could Not Open File', String(error));
              });
            })
            .catch((e) => window.toast.error('New File Failed', String(e))),
        );
      },
    });
  }

  function doRevealPath(targetPath) {
    Promise.resolve(invoke('project_reveal_path', { path: targetPath }))
      .catch((e) => window.toast.error('Reveal Failed', String(e)));
  }

  // One place that decides what "the view changed" means for the two shapes.
  // `&& pane.isLocal` (task-6 review, F9): the tree only ever has local
  // pseudo-panes today, so this is a defensive guard rather than a live fix
  // — but doNewFolder/doRename/doDelete are shared with the dual-pane REMOTE
  // side too, and this function has no other way to say "only route to the
  // tree for a local operation" if a future caller ever reaches it with
  // projectMode true and a non-local pane.
  function refreshAfterLocalOp(pane) {
    if (projectMode && projectTreeHandle && pane.isLocal) return projectTreeHandle.refresh(pane.currentPath);
    return loadEntries(pane);
  }

  function doNewFolder(pane) {
    showTextPromptDialog({
      title: 'New Folder',
      label: 'Name',
      initialValue: '',
      confirmLabel: 'Create',
      onConfirm: (name) => {
        const path = joinPath(pane.currentPath, name);
        const mkdirPromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localMkdir === 'function'
            ? filesDataService.localMkdir(invoke, path)
            : Promise.reject(new Error('Files data service unavailable: localMkdir')))
          : (filesDataService && typeof filesDataService.remoteMkdir === 'function'
            ? filesDataService.remoteMkdir(invoke, activeRemotePaneId, path)
            : Promise.reject(new Error('Files data service unavailable: remoteMkdir')));
        mkdirPromise
          .then(() => refreshAfterLocalOp(pane))
          .catch((e) => window.toast.error('New Folder Failed', String(e)));
      },
    });
  }

  function doRename(pane, entry) {
    showTextPromptDialog({
      title: 'Rename',
      label: 'Name',
      initialValue: entry.name,
      confirmLabel: 'Rename',
      onConfirm: (newName) => {
        if (newName === entry.name) return;
        const from = joinPath(pane.currentPath, entry.name);
        const to = joinPath(pane.currentPath, newName);
        const renamePromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localRename === 'function'
            ? filesDataService.localRename(invoke, from, to)
            : Promise.reject(new Error('Files data service unavailable: localRename')))
          : (filesDataService && typeof filesDataService.remoteRename === 'function'
            ? filesDataService.remoteRename(invoke, activeRemotePaneId, from, to)
            : Promise.reject(new Error('Files data service unavailable: remoteRename')));
        renamePromise
          .then(() => refreshAfterLocalOp(pane))
          .catch((e) => window.toast.error('Rename Failed', String(e)));
      },
    });
  }

  function doDelete(pane, entry) {
    showConfirmDialog({
      title: 'Delete',
      message: `Delete "${entry.name}"? This cannot be undone.`,
      confirmLabel: 'Delete',
      danger: true,
      onConfirm: () => {
        const path = joinPath(pane.currentPath, entry.name);
        const removePromise = pane.isLocal
          ? (filesDataService && typeof filesDataService.localRemove === 'function'
            ? filesDataService.localRemove(invoke, path, !!entry.is_dir)
            : Promise.reject(new Error('Files data service unavailable: localRemove')))
          : (filesDataService && typeof filesDataService.remoteRemove === 'function'
            ? filesDataService.remoteRemove(invoke, activeRemotePaneId, path, !!entry.is_dir)
            : Promise.reject(new Error('Files data service unavailable: remoteRemove')));
        removePromise
          .then(() => refreshAfterLocalOp(pane))
          .catch((e) => window.toast.error('Delete Failed', String(e)));
      },
    });
  }

  function doCopyPath(pane, entry) {
    const path = joinPath(pane.currentPath, entry.name);
    const copyPromise = filesDataService && typeof filesDataService.clipboardWriteText === 'function'
      ? filesDataService.clipboardWriteText(invoke, path)
      : Promise.reject(new Error('Files data service unavailable: clipboardWriteText'));
    Promise.resolve(copyPromise)
      .then(() => window.toast.success('Copied', 'Path copied to clipboard.'))
      .catch((e) => window.toast.error('Copy Failed', String(e)));
  }

  // ---------------------------------------------------------------------------
  // Row context menu
  // ---------------------------------------------------------------------------

  // Reuses the panel's own local operations against a tree row. The dual-pane
  // list keeps buildRowContextMenuItems (which carries the transfer entries a
  // tree has no use for); this is the tree's list, and both go through the
  // same filesPaneView.showRowContextMenu renderer.
  function buildTreeContextMenuItems(node) {
    // Where "create inside" operations land, and what Refresh reloads: the
    // node itself when it is a directory, its parent when it is a file (a
    // file has no listing of its own to refresh or create into).
    const containingDir = node.isDir ? node.path : node.parentPath;
    const reload = () => {
      if (!projectTreeHandle) return;
      projectTreeHandle.refresh(containingDir);
    };
    const dirPane = { isLocal: true, currentPath: containingDir, prefix: 'project' };
    // Rename/Delete/Copy Path act ON the node itself, so the pseudo-pane's
    // directory must be the node's own PARENT — doRename/doDelete/doCopyPath
    // all rebuild the target as joinPath(pane.currentPath, entry.name), and
    // that must land back on node.path. Reusing containingDir here would be
    // wrong for a directory node: containingDir IS node.path, so joining the
    // node's own name onto it would target a nonexistent child of itself
    // instead of the directory.
    const nodePane = { isLocal: true, currentPath: node.parentPath, prefix: 'project' };
    return [
      // 'add', not 'newFile': there is no newFile.svg in vendor/intellij-icons
      // (only 'newFolder' was ever vendored), and 'add' is the codebase's
      // existing "create new thing" glyph (see ssh-panel.js's New Connection /
      // Add Server Here) — using 'newFile' verbatim would render a broken
      // image icon in the menu.
      { icon: 'add', label: 'New File…', action: () => doNewFile(containingDir, reload) },
      { icon: 'newFolder', label: 'New Folder…', action: () => doNewFolder(dirPane) },
      { type: 'separator' },
      { icon: 'edit', label: 'Rename…', action: () => doRename(nodePane, { name: node.name, is_dir: node.isDir }) },
      { icon: 'remove', label: 'Delete', danger: true, action: () => doDelete(nodePane, { name: node.name, is_dir: node.isDir }) },
      { type: 'separator' },
      { icon: 'copy', label: 'Copy Path', action: () => doCopyPath(nodePane, { name: node.name }) },
      { label: 'Reveal in File Manager', action: () => doRevealPath(node.path) },
      { type: 'separator' },
      { icon: 'refresh', label: 'Refresh', action: reload },
    ];
  }

  // F13 (task-6 review): the tree-background counterpart to
  // buildTreeContextMenuItems — reachable when there is no row to
  // right-click at all (an empty project directory, or simply the empty
  // space below the last row), which previously left New File/New Folder
  // completely unreachable in that state. Root-scoped: New File/New Folder
  // always land directly in projectRoot, since there is no node to derive a
  // containing directory from.
  function buildRootContextMenuItems() {
    const reload = () => {
      if (projectTreeHandle) projectTreeHandle.refreshAll();
    };
    const rootPane = { isLocal: true, currentPath: projectRoot, prefix: 'project' };
    return [
      { icon: 'add', label: 'New File…', action: () => doNewFile(projectRoot, reload) },
      { icon: 'newFolder', label: 'New Folder…', action: () => doNewFolder(rootPane) },
      { type: 'separator' },
      { icon: 'refresh', label: 'Refresh', action: reload },
    ];
  }

  function buildRowContextMenuItems(pane, entry) {
    const noSession = !activeRemotePaneId;
    const sessionTitle = pane.isLocal
      ? 'Connect to an SSH session to upload files.'
      : 'Connect to an SSH session to download files.';

    const items = [
      { icon: 'newFolder', label: 'New Folder…', action: () => doNewFolder(pane) },
      { type: 'separator' },
      { icon: 'edit', label: 'Rename…', action: () => doRename(pane, entry) },
      { icon: 'remove', label: 'Delete', danger: true, action: () => doDelete(pane, entry) },
      { type: 'separator' },
      { icon: 'copy', label: 'Copy Path', action: () => doCopyPath(pane, entry) },
      { type: 'separator' },
    ];

    if (pane.isLocal) {
      items.push({
        label: 'Upload to remote host',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doUpload(entry),
      });
      items.push({
        label: 'Upload to path…',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doUploadToPath(entry),
      });
      if (entry && entry.is_dir) {
        items.push({
          label: 'Upload Folder',
          disabled: noSession,
          title: noSession ? sessionTitle : undefined,
          action: () => doUploadFolder(entry),
        });
      }
    } else {
      items.push({
        label: 'Download to local host',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doDownload(entry),
      });
      items.push({
        label: 'Download to path…',
        disabled: noSession,
        title: noSession ? sessionTitle : undefined,
        action: () => doDownloadToPath(entry),
      });
      if (entry && entry.is_dir) {
        items.push({
          label: 'Download Folder',
          disabled: noSession,
          title: noSession ? sessionTitle : undefined,
          action: () => doDownloadFolder(entry),
        });
      }
    }

    items.push({ type: 'separator' });
    items.push({ icon: 'refresh', label: 'Refresh', action: () => loadEntries(pane) });

    return items;
  }

  function showRowContextMenu(event, pane, entry) {
    if (!filesPaneView || typeof filesPaneView.showRowContextMenu !== 'function') {
      console.error('files-pane-view missing showRowContextMenu');
      return;
    }
    filesPaneView.showRowContextMenu(event, buildRowContextMenuItems(pane, entry));
  }

  // ---------------------------------------------------------------------------
  // Small single-field / confirm dialogs — render through the shared
  // window.tlDialog shell (app/ui/tl-dialog.js), which owns the overlay,
  // focus trap, Escape and backdrop dismissal.
  // ---------------------------------------------------------------------------

  // Only one files-panel dialog is ever open at a time (both are opened
  // from row/context-menu actions), so a single tracked handle is enough to
  // close "our own" dialog without touching any other module's.
  let activeFilesDialogHandle = null;

  function removeFilesOverlay() {
    if (activeFilesDialogHandle) {
      const handle = activeFilesDialogHandle;
      activeFilesDialogHandle = null;
      handle.close();
    }
  }

  function showTextPromptDialog(opts) {
    const o = opts || {};
    removeFilesOverlay();
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (activeFilesDialogHandle === handle) activeFilesDialogHandle = null;
      if (handle) handle.close();
    };
    const confirm = () => {
      const input = handle.el.querySelector('#fp-dlg-input');
      const value = input.value.trim();
      if (!value) { input.focus(); return; }
      dismiss();
      if (typeof o.onConfirm === 'function') o.onConfirm(value);
    };

    handle = window.tlDialog.open({
      title: o.title,
      ariaLabel: o.title,
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">${esc(o.label)}</span>
            <input type="text" class="tl-input" id="fp-dlg-input" value="${attr(o.initialValue || '')}" spellcheck="false" />
          </div>
        `;
        const input = bodyEl.querySelector('#fp-dlg-input');
        setTimeout(() => { input.focus(); input.select(); }, 50);
      },
      buttons: [
        { label: 'Cancel', onSelect: dismiss },
        { label: o.confirmLabel || 'OK', primary: true, onSelect: confirm },
      ],
      onClose: dismiss,
      // The old overlay-level key handler mapped Enter to confirm
      // regardless of which element had focus (document-level capture, so
      // it fired before a focused button's native Enter-triggers-click
      // could). A body-scoped bubble listener would miss Enter when focus
      // is on a footer button (the footer isn't a descendant of the body),
      // and that button would fire instead — e.g. Tab to Cancel, press
      // Enter, and the old code still confirmed. Listening on the
      // fully-built panel reproduces that "always wins" behavior.
      onOpen: (panelEl) => {
        panelEl.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          confirm();
        });
      },
    });
    activeFilesDialogHandle = handle;
  }

  function showConfirmDialog(opts) {
    const o = opts || {};
    removeFilesOverlay();
    if (!window.tlDialog || typeof window.tlDialog.open !== 'function') return;

    let handle = null;
    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (activeFilesDialogHandle === handle) activeFilesDialogHandle = null;
      if (handle) handle.close();
    };
    const confirm = () => {
      dismiss();
      if (typeof o.onConfirm === 'function') o.onConfirm();
    };

    handle = window.tlDialog.open({
      title: o.title,
      ariaLabel: o.title,
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="ssh-auth-message">${esc(o.message)}</div>`;
      },
      buttons: [
        { label: 'Cancel', onSelect: dismiss },
        { label: o.confirmLabel || 'OK', primary: true, danger: !!o.danger, onSelect: confirm },
      ],
      onClose: dismiss,
    });
    activeFilesDialogHandle = handle;
  }

  // ---------------------------------------------------------------------------
  // Transfer progress toasts
  // ---------------------------------------------------------------------------

  function handleTransferProgress(event) {
    if (!transferController || typeof transferController.handleTransferProgress !== 'function') {
      console.error('files-transfers missing handleTransferProgress controller');
      return;
    }
    transferController.handleTransferProgress(event);
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  const formatSize = window.utils.formatSize;
  const formatDate = window.utils.formatDate;

  function extOf(name) {
    if (!filesPaneStore || typeof filesPaneStore.extOf !== 'function') {
      console.error('files-pane-store missing extOf');
      return '';
    }
    return filesPaneStore.extOf(name);
  }

  const esc = window.utils.esc;
  const attr = window.utils.attr;

  // pollActiveRemotePaneCwd is exported only so test_sftp_connect.mjs can
  // drive the 600ms poll's callback directly (the harness stubs
  // setInterval/clearInterval to a no-op — see setupLogicHarness — so the
  // real timer never fires there); it is not part of the module's runtime
  // API surface otherwise.
  exports.filesPanel = {
    init, togglePanel, isHidden, onTabChanged, pinRemotePane, pollActiveRemotePaneCwd,
    isProjectMode,
    setProjectMode,
    projectTree,
    checkProjectRootPresence,
  };
})(window);
