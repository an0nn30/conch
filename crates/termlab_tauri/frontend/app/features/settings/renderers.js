// Settings feature — shared DOM helpers, dialog/standalone shells, theme
// preview, shortcut recording widgets, and section render dispatch.

(function initTermLabSettingsRenderers(global) {
  'use strict';

  const searchFeature = global.termlabSettingsFeatureSearch || {};

  // --- Shared layout helpers (reused by all section renderers) ---

  // Section header + rule, merged into one construct per the reference
  // (jvm-termlab-settings.png / METRICS.md "Settings window"): a plain-case
  // label followed by a 1px rule running to the right edge — the rule sits
  // BESIDE the label, not underneath it as a separate spacer line the way
  // the old two-call addSectionLabel()+addDivider() pair drew it.
  //
  // addDivider() is kept as a callable no-op: ~19 remaining call sites
  // across sections-*.js (out of Task 1's scope) still call
  // addDivider(container) between two addSectionLabel() calls, matching the
  // old "label, rows, divider, label, rows" convention. Since the rule is
  // now part of the *next* label's own row, that trailing addDivider() call
  // has nothing left to do — .tl-settings__section's own top margin (see
  // components/settings.css) recreates the old spacer's vertical gap.
  //
  // trailingEl is optional: a node inserted between the label and the rule,
  // for headers that need an inline action beside the label (e.g. Settings
  // > Plugins' "Installed Plugins … Rescan" — see plugins-section.js) while
  // still letting the rule reach the right edge. Returns the constructed
  // row so a caller can tag it (e.g. dataset.settingId) the same way the
  // old hand-built header div could.
  function addSectionLabel(container, text, trailingEl) {
    const row = document.createElement('div');
    row.className = 'tl-settings__section';
    const label = document.createElement('span');
    label.className = 'tl-settings__section-label';
    label.textContent = text;
    row.appendChild(label);
    if (trailingEl) row.appendChild(trailingEl);
    const rule = document.createElement('span');
    rule.className = 'tl-settings__rule';
    row.appendChild(rule);
    container.appendChild(row);
    return row;
  }

  function addDivider(_container) {
    // See addSectionLabel's comment above — intentionally inert.
  }

  function addRow(container, labelText, descText, controlEl) {
    const row = document.createElement('div');
    row.className = 'tl-settings__row';
    if (labelText) row.dataset.searchLabel = searchFeature.normalizeSearchText(labelText);
    if (descText) row.dataset.searchDesc = searchFeature.normalizeSearchText(descText);
    const left = document.createElement('div');
    const lbl = document.createElement('div');
    lbl.className = 'tl-settings__row-label';
    lbl.textContent = labelText;
    left.appendChild(lbl);
    if (descText) {
      const desc = document.createElement('div');
      desc.className = 'tl-settings__row-desc';
      desc.textContent = descText;
      left.appendChild(desc);
    }
    row.appendChild(left);
    row.appendChild(controlEl);
    container.appendChild(row);
    return row;
  }

  function setRowTarget(row, settingId) {
    if (row && settingId) row.dataset.settingId = settingId;
    return row;
  }

  function applyRowSearchHighlight(row, labelText, descText, query) {
    if (!row || !query) return;
    const labelEl = row.querySelector('.tl-settings__row-label');
    if (labelEl) {
      labelEl.textContent = '';
      searchFeature.appendHighlightedText(labelEl, labelText, query);
    }
    const descEl = row.querySelector('.tl-settings__row-desc');
    if (descEl && descText) {
      descEl.textContent = '';
      searchFeature.appendHighlightedText(descEl, descText, query);
    }
  }

  function addSearchInput(container, placeholder, value, onInput) {
    const wrap = document.createElement('div');
    wrap.className = 'settings-search-wrap';
    const input = document.createElement('input');
    input.type = 'search';
    input.className = 'settings-input settings-search-input';
    input.placeholder = placeholder;
    input.value = value || '';
    input.addEventListener('input', () => onInput(input.value));
    wrap.appendChild(input);
    container.appendChild(wrap);
    return input;
  }

  // --- Shared control helpers ---

  function makeInput(type, value, opts = {}) {
    const input = document.createElement('input');
    input.type = type;
    input.className = 'tl-input';
    input.value = value ?? '';
    if (opts.placeholder) input.placeholder = opts.placeholder;
    if (opts.step) input.step = opts.step;
    if (opts.min !== undefined) input.min = opts.min;
    if (opts.max !== undefined) input.max = opts.max;
    if (opts.style) input.style.cssText = opts.style;
    return input;
  }

  function makeToggleGroup(options, activeValue, onChange) {
    const group = document.createElement('div');
    group.className = 'tl-toggle-group';
    group.setAttribute('role', 'radiogroup');
    const setActive = (activeBtn) => {
      for (const child of group.children) {
        child.classList.remove('is-active');
        child.setAttribute('aria-checked', child === activeBtn ? 'true' : 'false');
      }
      activeBtn.classList.add('is-active');
      activeBtn.setAttribute('aria-checked', 'true');
    };
    for (const opt of options) {
      const btn = document.createElement('div');
      btn.className = 'tl-toggle-group__btn' + (opt.value === activeValue ? ' is-active' : '');
      btn.textContent = opt.label;
      btn.setAttribute('role', 'radio');
      btn.setAttribute('aria-checked', opt.value === activeValue ? 'true' : 'false');
      btn.tabIndex = 0;
      const activate = () => {
        onChange(opt.value);
        setActive(btn);
      };
      btn.addEventListener('click', activate);
      btn.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        activate();
      });
      group.appendChild(btn);
    }
    return group;
  }

  // Replaces the deleted makeSwitch() per the settled product decision
  // (2026-08-15): every boolean setting in Settings renders as a checkbox
  // now, not an iOS-style switch — the IntelliJ reference has no switch
  // control at all. Reuses the existing .tl-check component unstyled (no
  // visible label text: the row's own .tl-settings__row-label already
  // carries the setting's name in the row's left column) rather than
  // inventing new CSS, per this task's "never write bespoke CSS for
  // something a component already does" constraint.
  function makeCheckbox(checked, onChange) {
    const label = document.createElement('label');
    label.className = 'tl-check';
    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = checked;
    cb.addEventListener('change', () => onChange(cb.checked));
    label.appendChild(cb);
    return label;
  }

  // --- Theme preview helpers ---

  function span(cls, text) {
    const s = document.createElement('span');
    if (cls) s.className = cls;
    s.textContent = text;
    return s;
  }

  function line(...nodes) {
    const d = document.createElement('div');
    for (const n of nodes) {
      if (typeof n === 'string') d.appendChild(document.createTextNode(n));
      else d.appendChild(n);
    }
    return d;
  }

  function buildThemePreview() {
    const box = document.createElement('div');
    box.className = 'tl-settings__theme-preview';

    // "PREVIEW" label
    const label = document.createElement('div');
    label.textContent = 'PREVIEW';
    label.className = 'tl-settings__theme-preview-label tp-dim';
    box.appendChild(label);

    // Prompt line
    box.appendChild(line(
      span('tp-green tp-bold', 'user@termlab'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
      span('tp-fg', 'ls -la'),
    ));

    // total line
    box.appendChild(line(span('tp-fg', 'total 42')));

    // File listing entries: [permissions, links, user, group, size, date, name]
    const entries = [
      // [perm, links, user, group, size, date, name, nameClass]
      ['drwxr-xr-x', '5', 'user', 'staff', '160', 'Mar 20 10:01', '.', 'tp-blue tp-bold'],
      ['drwxr-xr-x', '8', 'user', 'staff', '256', 'Mar 19 09:00', '..', 'tp-blue tp-bold'],
      ['-rw-r--r--', '1', 'user', 'staff', '1234', 'Mar 20 10:01', '.gitignore', 'tp-yellow'],
      ['-rw-r--r--', '1', 'user', 'staff', '890', 'Mar 20 10:01', '.env', 'tp-yellow'],
      ['drwxr-xr-x', '3', 'user', 'staff', '96', 'Mar 20 10:01', 'src', 'tp-blue tp-bold'],
      ['-rwxr-xr-x', '1', 'user', 'staff', '8192', 'Mar 20 10:01', 'build.sh', 'tp-red tp-bold'],
      ['-rw-r--r--', '1', 'user', 'staff', '512', 'Mar 20 10:01', 'config.toml', 'tp-green'],
      ['-rw-r--r--', '1', 'user', 'staff', '256', 'Mar 20 10:01', 'README.md', 'tp-fg'],
    ];

    for (const [perm, links, user, group, size, date, name, nameClass] of entries) {
      box.appendChild(line(
        span('tp-dim', perm + ' '),
        span('tp-cyan', links + ' '),
        span('tp-dim', user + ' ' + group + ' '),
        span('tp-cyan', size.padStart(6) + ' '),
        span('tp-dim', date + ' '),
        span(nameClass, name),
      ));
    }

    // echo command line
    box.appendChild(line(
      span('tp-green tp-bold', 'user@termlab'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
      span('tp-magenta', 'echo'),
      span('tp-fg', ' '),
      span('tp-yellow', '"hello world"'),
    ));

    // output line
    box.appendChild(line(span('tp-fg', 'hello world')));

    // cursor prompt line
    const cursorLine = line(
      span('tp-green tp-bold', 'user@termlab'),
      span('tp-fg', ':'),
      span('tp-blue tp-bold', '~/projects'),
      span('tp-fg', ' $ '),
    );
    const cursor = document.createElement('span');
    cursor.className = 'tl-settings__theme-preview-cursor';
    cursor.textContent = ' ';
    cursorLine.appendChild(cursor);
    box.appendChild(cursorLine);

    // Swatch divider
    const dividerEl = document.createElement('div');
    dividerEl.className = 'tl-settings__theme-preview-divider';
    box.appendChild(dividerEl);

    // Normal swatches row
    const normalRow = document.createElement('div');
    normalRow.className = 'tl-settings__theme-preview-swatches tl-settings__theme-preview-swatches--normal';
    const normalClasses = ['tp-sw-black','tp-sw-red','tp-sw-green','tp-sw-yellow','tp-sw-blue','tp-sw-magenta','tp-sw-cyan','tp-sw-white'];
    for (const cls of normalClasses) {
      const sw = document.createElement('div');
      sw.className = cls + ' tl-settings__theme-preview-swatch';
      normalRow.appendChild(sw);
    }
    box.appendChild(normalRow);

    // Bright swatches row
    const brightRow = document.createElement('div');
    brightRow.className = 'tl-settings__theme-preview-swatches';
    const brightClasses = ['tp-sw-bright-black','tp-sw-bright-red','tp-sw-bright-green','tp-sw-bright-yellow','tp-sw-bright-blue','tp-sw-bright-magenta','tp-sw-bright-cyan','tp-sw-bright-white'];
    for (const cls of brightClasses) {
      const sw = document.createElement('div');
      sw.className = cls + ' tl-settings__theme-preview-swatch';
      brightRow.appendChild(sw);
    }
    box.appendChild(brightRow);

    return box;
  }

  function updateThemePreview(container, tc) {
    if (!tc) return;

    // Container background and border
    container.style.background = tc.background || '';
    container.style.borderColor = tc.tab_border || '';

    // Text color classes
    const colorMap = {
      '.tp-fg':      tc.foreground,
      '.tp-dim':     tc.dim_fg,
      '.tp-green':   tc.green,
      '.tp-blue':    tc.blue,
      '.tp-cyan':    tc.cyan,
      '.tp-red':     tc.red,
      '.tp-yellow':  tc.yellow,
      '.tp-magenta': tc.magenta,
    };
    for (const [sel, color] of Object.entries(colorMap)) {
      if (!color) continue;
      for (const el of container.querySelectorAll(sel)) {
        el.style.color = color;
      }
    }

    // Bold elements
    for (const el of container.querySelectorAll('.tp-bold')) {
      el.style.fontWeight = 'bold';
    }

    // Cursor block
    const cursorEl = container.querySelector('.tl-settings__theme-preview-cursor');
    if (cursorEl) {
      cursorEl.style.background = tc.cursor_color || tc.foreground || '';
      cursorEl.style.color = tc.cursor_text || tc.background || '';
    }

    // Normal swatches
    const normalSwatches = [
      ['.tp-sw-black',   tc.black],
      ['.tp-sw-red',     tc.red],
      ['.tp-sw-green',   tc.green],
      ['.tp-sw-yellow',  tc.yellow],
      ['.tp-sw-blue',    tc.blue],
      ['.tp-sw-magenta', tc.magenta],
      ['.tp-sw-cyan',    tc.cyan],
      ['.tp-sw-white',   tc.white],
    ];
    for (const [sel, color] of normalSwatches) {
      if (!color) continue;
      const el = container.querySelector(sel);
      if (el) el.style.background = color;
    }

    // Bright swatches
    const brightSwatches = [
      ['.tp-sw-bright-black',   tc.bright_black],
      ['.tp-sw-bright-red',     tc.bright_red],
      ['.tp-sw-bright-green',   tc.bright_green],
      ['.tp-sw-bright-yellow',  tc.bright_yellow],
      ['.tp-sw-bright-blue',    tc.bright_blue],
      ['.tp-sw-bright-magenta', tc.bright_magenta],
      ['.tp-sw-bright-cyan',    tc.bright_cyan],
      ['.tp-sw-bright-white',   tc.bright_white],
    ];
    for (const [sel, color] of brightSwatches) {
      if (!color) continue;
      const el = container.querySelector(sel);
      if (el) el.style.background = color;
    }

    // Swatch divider border
    const divider = container.querySelector('.tl-settings__theme-preview-divider');
    if (divider) divider.style.borderTopColor = tc.active_highlight || '';
  }

  // --- Keyboard shortcut display/recording ---

  const isMac = typeof navigator !== 'undefined' && navigator.platform.includes('Mac');

  /** Convert config shortcut string to display string, e.g. "cmd+shift+t" -> "⌘ ⇧ T" */
  function formatShortcut(combo) {
    if (!combo) return '';
    const parts = combo.split('+');
    const display = [];
    for (const p of parts) {
      switch (p) {
        case 'cmd':   display.push(isMac ? '⌘' : 'Ctrl'); break;
        case 'shift': display.push('⇧'); break;
        case 'alt':   display.push(isMac ? '⌥' : 'Alt'); break;
        case 'ctrl':  display.push(isMac ? '⌃' : 'Ctrl'); break;
        default:      display.push(p.toUpperCase()); break;
      }
    }
    return display.join(' ');
  }

  /** Normalize a keydown event into config format, e.g. "cmd+shift+z" */
  function normalizeKeyEvent(e) {
    const parts = [];
    if (e.metaKey) parts.push('cmd');
    if (e.ctrlKey) parts.push('ctrl');
    if (e.altKey) parts.push('alt');
    if (e.shiftKey) parts.push('shift');
    // Ignore bare modifier keys
    const key = e.key.toLowerCase();
    if (['meta', 'control', 'alt', 'shift'].includes(key)) return null;
    parts.push(key);
    return parts.join('+');
  }

  function shortcutText(value) {
    const formatted = formatShortcut(value);
    return formatted || 'Unassigned';
  }

  /**
   * Shortcut-recording controller.
   * deps: { getShortcutValue, setShortcutValue, registerGlobalKeyHandler }
   */
  function createShortcutRecorder(deps) {
    const d = deps || {};

    // Currently recording shortcut state
    let recordingEl = null;
    let recordingRef = null;
    let recordingUnregister = null;

    function stopRecording() {
      if (recordingEl) {
        recordingEl.classList.remove('is-recording');
        recordingEl.textContent = shortcutText(d.getShortcutValue(recordingRef));
      }
      if (typeof recordingUnregister === 'function') {
        recordingUnregister();
        recordingUnregister = null;
      }
      recordingEl = null;
      recordingRef = null;
    }

    function startRecording(el, settingsRef) {
      // Stop any existing recording first
      stopRecording();

      recordingEl = el;
      recordingRef = settingsRef;
      el.classList.add('is-recording');
      el.textContent = 'Press keys...';

      // Registered above tl-dialog's own Escape handler (priority 225 — see
      // registerEscape() in app/ui/tl-dialog.js; same pattern as the command
      // palette's priority-260 registration in command-palette-runtime.js)
      // so THIS handler's Escape branch — which only cancels the in-progress
      // recording — wins while recording is active, instead of tl-dialog's
      // generic Escape closing the whole modal and discarding unsaved edits
      // (phase 5b review finding 2). isActive is scoped to "currently
      // recording", so this registration is a no-op the rest of the time and
      // tl-dialog's Escape still closes the modal normally.
      recordingUnregister = d.registerGlobalKeyHandler('settings-shortcut-recorder', (e) => {
        e.preventDefault();
        e.stopPropagation();

        // Escape cancels recording
        if (e.key === 'Escape') {
          stopRecording();
          return true;
        }

        const combo = normalizeKeyEvent(e);
        if (!combo) return true; // bare modifier, keep waiting

        d.setShortcutValue(settingsRef, combo);
        // Shortcut edits land in pendingSettings via the keyboard router's
        // global handler above, not a DOM input/change event on any element
        // inside the dialog — the footer's Apply-dirty-state listener (see
        // renderDialogShell/renderStandaloneShell) only hears bubbled
        // input/change/click, so it would otherwise miss a shortcut-only
        // edit entirely. Broadcast on `document` so it's heard regardless of
        // which shell (modal or standalone) is currently open.
        if (typeof document !== 'undefined' && typeof document.dispatchEvent === 'function') {
          document.dispatchEvent(new CustomEvent('termlab-settings-changed'));
        }
        stopRecording();
        return true;
      }, () => !!recordingEl && !!recordingRef, 230);
    }

    function makeShortcutKeyBox(ref) {
      const keyBox = document.createElement('span');
      keyBox.className = 'tl-settings__shortcut-key';
      keyBox.setAttribute('role', 'button');
      keyBox.tabIndex = 0;
      keyBox.setAttribute('aria-label', 'Record shortcut');
      keyBox.textContent = shortcutText(d.getShortcutValue(ref));
      keyBox.addEventListener('click', () => startRecording(keyBox, ref));
      keyBox.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        startRecording(keyBox, ref);
      });
      return keyBox;
    }

    return {
      makeShortcutKeyBox,
      startRecording,
      stopRecording,
      isRecording: () => !!recordingEl,
    };
  }

  // --- Search jump highlighting ---

  function applyPendingSettingsJump(root, jump) {
    if (!jump || !root) return false;
    let row = null;
    if (jump.targetId) {
      row = root.querySelector(`[data-setting-id="${jump.targetId}"]`);
    }
    if (!row && jump.label) {
      const normalized = searchFeature.normalizeSearchText(jump.label);
      row = root.querySelector(`.tl-settings__row[data-search-label="${normalized}"]`);
    }
    if (!row && jump.query) {
      const q = searchFeature.normalizeSearchText(jump.query);
      row = Array.from(root.querySelectorAll('.tl-settings__row')).find((el) => {
        const label = el.dataset.searchLabel || '';
        const desc = el.dataset.searchDesc || '';
        return label.includes(q) || desc.includes(q);
      }) || null;
    }
    if (!row && jump.targetId && jump.targetId.startsWith('plugin-setting:')) {
      const parts = jump.targetId.split(':');
      const widgetId = parts.length >= 3 ? parts[parts.length - 1] : '';
      if (widgetId) {
        const widget = root.querySelector(`[data-pw-id="${CSS.escape(widgetId)}"]`);
        if (widget) {
          const highlightTarget =
            widget.closest(`[data-plugin-setting-id="${CSS.escape(widgetId)}"]`)
            || widget.closest('.plugin-settings-content')
            || widget;
          highlightTarget.scrollIntoView({ behavior: 'smooth', block: 'center' });
          if (typeof widget.focus === 'function' && !widget.disabled) {
            widget.focus({ preventScroll: true });
          }
          highlightTarget.classList.remove('plugin-setting-jump-highlight');
          void highlightTarget.offsetWidth;
          highlightTarget.classList.add('plugin-setting-jump-highlight');
          return true;
        }
      }
    }
    if (row) {
      row.scrollIntoView({ behavior: 'smooth', block: 'center' });
      row.classList.remove('settings-row-jump-highlight');
      void row.offsetWidth;
      row.classList.add('settings-row-jump-highlight');
      return true;
    }
    return false;
  }

  // --- Dialog / standalone window shells ---

  // Shared by both shells: the sidebar+content two-pane body. Appended
  // directly as the modal's tl-dialog__body content (Step 1: "the standalone
  // window ... shares every inner class").
  function buildSettingsBody(d) {
    const shell = document.createElement('div');
    shell.className = 'tl-settings';

    const sidebar = document.createElement('div');
    sidebar.className = 'tl-settings__sidebar tl-scroll';
    sidebar.id = 'settings-sidebar';
    d.renderSidebarInto(sidebar);
    shell.appendChild(sidebar);

    const content = document.createElement('div');
    content.className = 'tl-settings__content tl-scroll';
    content.id = 'settings-content';
    shell.appendChild(content);

    return shell;
  }

  // Wires the "press any printable key while nothing text-like is focused ->
  // jump into and type into the sidebar search box" behavior onto whichever
  // root element owns the shell's keydown events (the dialog panel for the
  // modal shell, `root` for standalone). Unchanged from the pre-tl-dialog
  // version except the id/class lookups now target the renamed elements.
  function wireTypeToSearch(scopeEl, containsScope, d) {
    scopeEl.addEventListener('keydown', (e) => {
      if (d.isRecording()) return;
      if (!searchFeature.isPrintableKeyEvent(e)) return;
      const active = document.activeElement;
      if (active && searchFeature.isTextLikeElement(active) && containsScope(active)) return;
      const input = scopeEl.querySelector('.tl-settings__search');
      if (!input) return;
      e.preventDefault();
      e.stopPropagation();
      input.focus();
      input.value = (input.value || '') + e.key;
      d.setSidebarQuery(input.value);
      const sidebarEl = document.getElementById('settings-sidebar');
      if (sidebarEl) d.renderSidebarInto(sidebarEl);
      const nextInput = scopeEl.querySelector('.tl-settings__search');
      if (nextInput) {
        nextInput.focus();
        nextInput.setSelectionRange(nextInput.value.length, nextInput.value.length);
      }
    }, true);
  }

  // Finds the Apply button among a footer's buttons by label (tl-dialog's
  // buildFooterButton doesn't expose ids/refs, and label text is a stable,
  // caller-controlled string here) and keeps its disabled state in sync with
  // d.isDirty() as the user edits: bubbled input/change/click cover ordinary
  // controls (text fields, switches, toggle groups — all mutate
  // pendingSettings synchronously in their own click/change handler, which
  // runs before this bubble-phase listener sees the event), and the
  // 'termlab-settings-changed' custom event covers the keyboard-shortcut
  // recorder's out-of-band edits (see createShortcutRecorder above). Does
  // NOT observe plugin-settings widgets, which mutate server-side drafts
  // rather than pendingSettings (see store.js's isDirty() comment) — a known
  // gap, not a regression from the pre-Task-1 shell, which had no
  // dirty-tracking at all.
  function wireApplyDirtyTracking(footerEl, scopeEl, d) {
    const applyBtn = Array.from(footerEl.querySelectorAll('.tl-btn')).find((btn) => btn.textContent === 'Apply') || null;
    if (!applyBtn) return { refresh: () => {}, dispose: () => {} };
    // tl-dialog.js's buildFooterButton() only sets aria-disabled once, at
    // build time, from the static spec passed to tlDialog.open() (Apply
    // always starts disabled). Toggling just the live `disabled` property
    // here — as this used to do — desyncs the two: a screen reader keeps
    // reporting Apply as permanently disabled even once the store goes
    // dirty and it becomes clickable (phase 5b review finding 6). Set both
    // on every refresh so aria-disabled always matches the property that is
    // actually gating the click handler (see buildFooterButton's own
    // comment on why btn.disabled, not spec.disabled, is authoritative).
    const refresh = () => {
      const disabled = !d.isDirty();
      applyBtn.disabled = disabled;
      applyBtn.setAttribute('aria-disabled', disabled ? 'true' : 'false');
    };
    refresh();
    scopeEl.addEventListener('input', refresh);
    scopeEl.addEventListener('change', refresh);
    scopeEl.addEventListener('click', refresh);
    document.addEventListener('termlab-settings-changed', refresh);
    const dispose = () => document.removeEventListener('termlab-settings-changed', refresh);
    return { refresh, dispose };
  }

  function helpFooterButtonSpec() {
    return {
      label: '?',
      onSelect: () => {
        if (global.toast && typeof global.toast.info === 'function') {
          global.toast.info('Settings', 'Help documentation is not available yet.');
        }
      },
    };
  }

  /**
   * Build the in-app modal dialog shell via tlDialog.open() and attach it to
   * document.body.
   * deps: {
   *   close, applyKeepOpen, applyAndClose, isDirty, renderSidebarInto,
   *   isRecording, setSidebarQuery, onClose,
   * }
   */
  function renderDialogShell(deps) {
    const d = deps || {};
    const body = buildSettingsBody(d);

    const handle = global.tlDialog.open({
      title: 'Settings',
      ariaLabel: 'Settings',
      size: 'lg',
      body,
      footerStart: [helpFooterButtonSpec()],
      buttons: [
        { label: 'Cancel', onSelect: () => d.close() },
        {
          label: 'Apply',
          disabled: !d.isDirty(),
          onSelect: () => { Promise.resolve(d.applyKeepOpen()).then(() => tracking.refresh()); },
        },
        { label: 'OK', primary: true, onSelect: () => d.applyAndClose() },
      ],
      // Escape and backdrop-dismiss close the dialog through tl-dialog's own
      // internal close(), never through this module's Cancel/OK onSelect —
      // so both this module's tracking cleanup AND the caller's store/plugin
      // -draft cleanup (deps.onClose, e.g. settings.js's
      // handleModalDialogClosed) have to run from here to fire on every
      // close path, not just the button ones.
      onClose: (result) => {
        tracking.dispose();
        if (typeof d.onClose === 'function') d.onClose(result);
      },
    });

    const panel = handle.el;
    // tl-dialog's own .tl-dialog__footer (components/dialog.css) carries no
    // border — right for the generic "Modal dialog shell" pattern (Add SSH
    // Host/Tunnel, Unlock Vault: METRICS.md notes only "generous padding
    // above the button row", no divider). The Settings reference capture
    // (jvm-termlab-settings.png) shows a full-width divider above its
    // Cancel/Apply/OK row that those other dialogs don't have, so it's
    // scoped to Settings via this extra class rather than added to the
    // shared component (phase 5b review finding 9) — see the matching class
    // added in renderStandaloneShell below and its CSS in
    // components/settings.css.
    const dialogFooterEl = panel.querySelector('.tl-dialog__footer');
    if (dialogFooterEl) dialogFooterEl.classList.add('tl-settings__footer');
    const footerEl = panel.querySelector('.tl-dialog__footer-end') || panel;
    const tracking = wireApplyDirtyTracking(footerEl, panel, d);

    wireTypeToSearch(panel, (el) => panel.contains(el), d);

    return handle;
  }

  /**
   * Build the standalone-window shell (no overlay, no modal) into rootEl,
   * sharing the same .tl-settings inner markup and .tl-dialog__footer*
   * button classes as the modal shell.
   * deps: {
   *   close, applyKeepOpen, applyAndClose, isDirty, renderSidebarInto,
   *   isRecording, setSidebarQuery,
   * }
   */
  function renderStandaloneShell(root, deps) {
    const d = deps || {};
    root.innerHTML = '';

    // Title bar (also serves as drag region) — standalone-only; the modal
    // shell gets its title from tlDialog.open({ title }) instead.
    const title = document.createElement('div');
    title.className = 'tl-settings__title';
    title.textContent = 'Settings';
    title.setAttribute('data-tauri-drag-region', '');
    root.appendChild(title);

    root.appendChild(buildSettingsBody(d));

    const footer = document.createElement('div');
    // tl-settings__footer: see renderDialogShell's matching comment above —
    // the Settings-only top divider (finding 9), not part of the shared
    // .tl-dialog__footer component.
    footer.className = 'tl-dialog__footer tl-settings__footer';
    const startEl = document.createElement('div');
    startEl.className = 'tl-dialog__footer-start';
    const helpBtn = document.createElement('button');
    helpBtn.type = 'button';
    helpBtn.className = 'tl-btn';
    helpBtn.textContent = '?';
    const helpSpec = helpFooterButtonSpec();
    helpBtn.addEventListener('click', helpSpec.onSelect);
    startEl.appendChild(helpBtn);
    footer.appendChild(startEl);

    const endEl = document.createElement('div');
    endEl.className = 'tl-dialog__footer-end';
    const cancelBtn = document.createElement('button');
    cancelBtn.type = 'button';
    cancelBtn.className = 'tl-btn';
    cancelBtn.textContent = 'Cancel';
    cancelBtn.addEventListener('click', () => d.close());
    const applyBtn = document.createElement('button');
    applyBtn.type = 'button';
    applyBtn.className = 'tl-btn';
    applyBtn.textContent = 'Apply';
    applyBtn.addEventListener('click', () => { Promise.resolve(d.applyKeepOpen()).then(() => tracking.refresh()); });
    const okBtn = document.createElement('button');
    okBtn.type = 'button';
    okBtn.className = 'tl-btn tl-btn--primary';
    okBtn.textContent = 'OK';
    okBtn.addEventListener('click', () => d.applyAndClose());
    endEl.appendChild(cancelBtn);
    endEl.appendChild(applyBtn);
    endEl.appendChild(okBtn);
    footer.appendChild(endEl);
    root.appendChild(footer);

    const tracking = wireApplyDirtyTracking(endEl, root, d);
    wireTypeToSearch(root, () => true, d);
  }

  // --- Section render dispatch ---

  /**
   * Build the per-section render dispatcher.
   * deps: {
   *   store, recorder, getInvoke, renderCurrentSection, renderSidebarInto,
   *   refreshPluginInventory, confirmPluginPermissions, invalidateCommandPaletteCache
   * }
   */
  function createSectionRenderers(deps) {
    const d = deps || {};
    const store = d.store;
    const recorder = d.recorder;

    function moduleUnavailable(label) {
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error('Settings Error', label + ' section module is unavailable.');
      }
    }

    function renderAppearance(c) {
      const settingsSectionsAppearance = global.termlabSettingsSectionsAppearance || {};
      if (settingsSectionsAppearance && typeof settingsSectionsAppearance.renderAppearance === 'function') {
        const handled = settingsSectionsAppearance.renderAppearance(c, {
          pendingSettings: store.getPendingSettings(),
          cachedThemes: store.getCachedThemes(),
          cachedFonts: store.getCachedFonts(),
          addSectionLabel,
          addRow,
          setRowTarget,
          addDivider,
          buildThemePreview,
          updateThemePreview,
          invoke: d.getInvoke(),
          makeCheckbox,
          makeInput,
          makeToggleGroup,
        });
        if (handled) return;
      }
      moduleUnavailable('Appearance');
    }

    function renderWindow(c) {
      const settingsSectionsWindow = global.termlabSettingsSectionsWindow || {};
      if (typeof settingsSectionsWindow.renderWindow === 'function') {
        const handled = settingsSectionsWindow.renderWindow(c, {
          pendingSettings: store.getPendingSettings(),
          addSectionLabel,
          addRow,
          setRowTarget,
          addDivider,
          makeCheckbox,
          makeInput,
        });
        if (handled) return;
      }
      moduleUnavailable('Window');
    }

    function renderEditor(c) {
      const settingsSectionsEditor = global.termlabSettingsSectionsEditor || {};
      if (typeof settingsSectionsEditor.renderEditor === 'function') {
        const handled = settingsSectionsEditor.renderEditor(c, {
          pendingSettings: store.getPendingSettings(),
          addSectionLabel,
          addRow,
          setRowTarget,
          addDivider,
          makeCheckbox,
        });
        if (handled) return;
      }
      moduleUnavailable('Editor');
    }

    function renderKeyboard(c) {
      const settingsSectionsKeyboard = global.termlabSettingsSectionsKeyboard || {};
      if (settingsSectionsKeyboard && typeof settingsSectionsKeyboard.renderKeyboard === 'function') {
        const handled = settingsSectionsKeyboard.renderKeyboard(c, {
          stopRecording: () => recorder.stopRecording(),
          addSearchInput,
          normalizeSearchText: searchFeature.normalizeSearchText,
          getFuzzyMatchScore: searchFeature.getFuzzyMatchScore,
          getKeyboardSearchQuery: () => store.getKeyboardSearchQuery(),
          setKeyboardSearchQuery: (value) => {
            store.setKeyboardSearchQuery(value);
          },
          renderCurrentSection: () => d.renderCurrentSection(),
          KEYBOARD_CORE_GROUPS: store.KEYBOARD_CORE_GROUPS,
          KEYBOARD_CORE_LABELS: store.KEYBOARD_CORE_LABELS,
          getPendingKeyboardMap: () => store.getPendingKeyboardMap(),
          addSectionLabel,
          addRow,
          setRowTarget,
          applyRowSearchHighlight,
          addDivider,
          makeShortcutKeyBox: (ref) => recorder.makeShortcutKeyBox(ref),
          getToolWindowItems: () => (
            global.toolWindowManager && typeof global.toolWindowManager.listWindows === 'function'
              ? global.toolWindowManager.listWindows().slice().sort((a, b) => {
                  const typeCmp = String(a.type || '').localeCompare(String(b.type || ''));
                  if (typeCmp !== 0) return typeCmp;
                  return String(a.title || '').localeCompare(String(b.title || ''));
                })
              : []
          ),
          getPluginMenuItems: () => store.getCachedPluginMenuItems() || [],
        });
        if (handled) return;
      }
      moduleUnavailable('Keyboard');
    }

    function renderTerminal(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderTerminal !== 'function') {
        moduleUnavailable('Terminal');
        return;
      }
      settingsSectionsTerminal.renderTerminal(c, {
        pendingSettings: store.getPendingSettings(),
        cachedFonts: store.getCachedFonts(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeInput,
      });
    }

    function renderShell(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderShell !== 'function') {
        moduleUnavailable('Shell');
        return;
      }
      settingsSectionsTerminal.renderShell(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeInput,
      });
    }

    function renderCursor(c) {
      const settingsSectionsTerminal = global.termlabSettingsSectionsTerminal || {};
      if (!settingsSectionsTerminal || typeof settingsSectionsTerminal.renderCursor !== 'function') {
        moduleUnavailable('Cursor');
        return;
      }
      settingsSectionsTerminal.renderCursor(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeCheckbox,
        makeToggleGroup,
      });
    }

    function renderAdvanced(c) {
      const settingsSectionsBasic = global.termlabSettingsSectionsBasic || {};
      if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderAdvanced !== 'function') {
        moduleUnavailable('Advanced');
        return;
      }
      settingsSectionsBasic.renderAdvanced(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addDivider,
        addRow,
        setRowTarget,
        makeCheckbox,
        makeInput,
      });
    }

    function renderFiles(c) {
      const settingsSectionsBasic = global.termlabSettingsSectionsBasic || {};
      if (!settingsSectionsBasic || typeof settingsSectionsBasic.renderFiles !== 'function') {
        moduleUnavailable('Files');
        return;
      }
      settingsSectionsBasic.renderFiles(c, {
        pendingSettings: store.getPendingSettings(),
        addSectionLabel,
        addRow,
        setRowTarget,
        makeCheckbox,
      });
    }

    function renderPlugins(c) {
      const settingsPluginsSection = global.termlabSettingsPluginsSection || {};
      if (settingsPluginsSection && typeof settingsPluginsSection.createRenderer === 'function') {
        const renderer = settingsPluginsSection.createRenderer({
          invoke: d.getInvoke(),
          getPendingSettings: () => store.getPendingSettings(),
          getCachedPlugins: () => store.getCachedPlugins(),
          setCachedPlugins: (next) => store.setCachedPlugins(next),
          setCachedPluginMenuItems: (next) => store.setCachedPluginMenuItems(next),
          setCachedPluginSettingsSections: (next) => store.setCachedPluginSettingsSections(next),
          refreshPluginInventory: () => d.refreshPluginInventory(),
          onPluginInventoryUpdated: () => {
            if (!store.getSectionById(store.getCurrentSection())) {
              store.setCurrentSection('plugins');
            }
            const sidebar = document.getElementById('settings-sidebar');
            if (sidebar) d.renderSidebarInto(sidebar);
            d.renderCurrentSection();
          },
          confirmPluginPermissions: (pluginName, permissions) => d.confirmPluginPermissions(pluginName, permissions),
          invalidateCommandPaletteCache: (reason) => d.invalidateCommandPaletteCache(reason),
          addSectionLabel,
          addDivider,
          addRow,
          setRowTarget,
          makeInput,
          makeCheckbox,
        });
        renderer.renderPlugins(c);
        return;
      }

      const fallback = document.createElement('div');
      fallback.className = 'settings-row-desc';
      fallback.textContent = 'Plugin settings UI module is unavailable.';
      c.appendChild(fallback);
    }

    function renderPluginSettings(c, sectionKey) {
      const section = store.getPluginSettingsSectionByKey(sectionKey);
      if (!section) {
        const fallback = document.createElement('div');
        fallback.className = 'settings-row-desc';
        fallback.textContent = 'Plugin settings section is unavailable.';
        c.appendChild(fallback);
        return;
      }

      const host = document.createElement('div');
      host.className = 'plugin-settings-content';
      host.dataset.pluginName = section.plugin_name || '';
      host.dataset.pluginViewId = section.view_id || '';
      c.appendChild(host);

      const loading = document.createElement('div');
      loading.className = 'settings-row-desc';
      loading.textContent = 'Loading plugin settings…';
      host.appendChild(loading);

      const pluginWidgets = global.pluginWidgets;
      if (!pluginWidgets || typeof pluginWidgets.renderWidgets !== 'function') {
        host.innerHTML = '';
        const missing = document.createElement('div');
        missing.className = 'settings-row-desc';
        missing.textContent = 'Plugin widget runtime is unavailable.';
        host.appendChild(missing);
        return;
      }

      const pluginName = section.plugin_name || '';
      const viewId = section.view_id || section.section_id || '';
      d.getInvoke()('request_plugin_render', { pluginName, viewId })
        .then((widgetsJson) => {
          host.innerHTML = '';
          pluginWidgets.renderWidgets(host, widgetsJson || '[]', pluginName, viewId);
          const jump = store.getPendingSettingsJump();
          if (jump && jump.section === sectionKey) {
            requestAnimationFrame(() => {
              const root = document.getElementById('settings-content');
              if (!root) return;
              if (applyPendingSettingsJump(root, store.getPendingSettingsJump())) {
                store.clearPendingSettingsJump();
              }
            });
          }
        })
        .catch((error) => {
          host.innerHTML = '';
          const failed = document.createElement('div');
          failed.className = 'settings-row-desc';
          failed.textContent = 'Failed to load plugin settings UI: ' + String(error);
          host.appendChild(failed);
          const jump = store.getPendingSettingsJump();
          if (jump && jump.section === sectionKey) {
            store.clearPendingSettingsJump();
          }
        });
    }

    function renderSection(content, sectionId) {
      switch (sectionId) {
        case 'appearance': renderAppearance(content); break;
        case 'keyboard': renderKeyboard(content); break;
        case 'files': renderFiles(content); break;
        case 'window': renderWindow(content); break;
        case 'editor': renderEditor(content); break;
        case 'terminal': renderTerminal(content); break;
        case 'shell': renderShell(content); break;
        case 'cursor': renderCursor(content); break;
        case 'plugins': renderPlugins(content); break;
        case 'advanced': renderAdvanced(content); break;
        default:
          if (store.isPluginSettingsSectionId(sectionId)) {
            renderPluginSettings(content, sectionId);
            break;
          }
          renderAppearance(content);
          break;
      }
    }

    return { renderSection };
  }

  global.termlabSettingsRenderers = {
    addSectionLabel,
    addDivider,
    addRow,
    setRowTarget,
    applyRowSearchHighlight,
    addSearchInput,
    makeInput,
    makeToggleGroup,
    makeCheckbox,
    buildThemePreview,
    updateThemePreview,
    formatShortcut,
    normalizeKeyEvent,
    shortcutText,
    createShortcutRecorder,
    applyPendingSettingsJump,
    renderDialogShell,
    renderStandaloneShell,
    createSectionRenderers,
  };
})(window);
