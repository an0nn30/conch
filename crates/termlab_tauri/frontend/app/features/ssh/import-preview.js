// Import preview — the conflict-resolution table shown between the share
// bundle's vault step and the actual `share_import_apply` call.
//
// `share_import_plan` (share_commands.rs) already ran the real import
// planner and reports, per row, which of four conflict statuses it hit and
// what the planner would do by default. This module renders that as a
// table the user can override row-by-row or in bulk, then hands back the
// `ImportDecision[]` array `share_import_apply` expects.
//
// Reuses the export picker's bounded-list shell (`.tl-picker__box` inside a
// `.tl-picker`) and its filter's matching rule (`window.termlabExportPicker
// .matchesFilter`) rather than building a parallel filterable-list
// component — see app/features/ssh/export-picker.js.
(function (exports) {
  'use strict';

  function esc(s) {
    return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));
  }

  // Actions-per-status table (Global Constraints, task-5 brief). Offering an
  // action outside this table is a defect — e.g. Replace on a
  // label_collision row would overwrite an unrelated local item that
  // happens to share this item's id namespace but not its label.
  const ACTIONS_BY_STATUS = {
    new: ['add', 'skip'],
    same_id: ['replace', 'skip'],
    label_collision: ['add', 'rename', 'skip'],
    reference_broken: ['skip'],
  };

  const ACTION_LABEL = { add: 'Add', replace: 'Replace', skip: 'Skip', rename: 'Rename' };
  const BULK_ACTION_LABEL = { add: 'Add all', replace: 'Replace all', skip: 'Skip all', rename: 'Rename all' };
  const KIND_LABEL = { host: 'Host', tunnel: 'Tunnel', credential: 'Credential' };

  const STATUS_ORDER = ['new', 'same_id', 'label_collision', 'reference_broken'];
  const STATUS_META = {
    new: { pill: 'New', noun: (n) => `${n} new` },
    same_id: { pill: 'Already exists', noun: (n) => `${n} already exist${n === 1 ? 's' : ''}` },
    label_collision: { pill: 'Name conflict', noun: (n) => `${n} name conflict${n === 1 ? '' : 's'}` },
    reference_broken: { pill: 'Broken reference', noun: (n) => `${n} broken reference${n === 1 ? '' : 's'}` },
  };

  function statusMeta(status) {
    return STATUS_META[status] || { pill: status, noun: (n) => `${n} ${status}` };
  }

  function actionsFor(status) {
    return ACTIONS_BY_STATUS[status] || ['skip'];
  }

  /** Counts decisions by action, in the fixed order add/replace/skip/rename
   * — matching the order the footer always displays them in regardless of
   * which counts are zero. Pure; exported for tests. */
  function summarise(decisions) {
    const list = Array.isArray(decisions) ? decisions : [];
    if (!list.length) return 'Nothing to import';
    const counts = { add: 0, replace: 0, skip: 0, rename: 0 };
    for (const d of list) {
      const action = d && d.action;
      if (Object.prototype.hasOwnProperty.call(counts, action)) counts[action] += 1;
    }
    const parts = [];
    if (counts.add) parts.push(`${counts.add} new`);
    if (counts.replace) parts.push(`${counts.replace} replace`);
    if (counts.skip) parts.push(`${counts.skip} skip`);
    if (counts.rename) parts.push(`${counts.rename} rename`);
    return parts.length ? parts.join(', ') : 'Nothing to import';
  }

  /** Appends " (2)", " (3)", … until the name is free of `existingLabels`.
   * Pure; exported for tests. */
  function suggestRename(label, existingLabels) {
    const taken = new Set((Array.isArray(existingLabels) ? existingLabels : []).map(String));
    const base = String(label == null ? '' : label);
    if (!taken.has(base)) return base;
    let n = 2;
    let candidate = `${base} (${n})`;
    while (taken.has(candidate)) {
      n += 1;
      candidate = `${base} (${n})`;
    }
    return candidate;
  }

  function normalizeRow(row) {
    const r = row || {};
    return {
      kind: r.kind,
      id: r.id,
      label: r.label == null ? '' : String(r.label),
      detail: r.detail == null ? '' : String(r.detail),
      status: r.status,
      default_action: r.default_action,
    };
  }

  function kindLabel(kind) {
    return KIND_LABEL[kind] || kind;
  }

  function rowHtml(row) {
    const allowed = actionsFor(row.status);
    const disabled = row.status === 'reference_broken';
    const defaultAction = allowed.indexOf(row.default_action) !== -1 ? row.default_action : allowed[0];
    const options = allowed
      .map((a) => `<option value="${a}"${a === defaultAction ? ' selected' : ''}>${esc(ACTION_LABEL[a])}</option>`)
      .join('');
    const meta = statusMeta(row.status);
    const haystack = [row.label, row.detail, kindLabel(row.kind), meta.pill].filter(Boolean).join(' ');
    return `<div class="tl-picker__row" data-role="row" data-kind="${esc(row.kind)}" data-id="${esc(row.id)}" data-status="${esc(row.status)}" data-search="${esc(haystack)}">`
      + `<span class="tl-picker__row-detail">${esc(kindLabel(row.kind))}</span>`
      + `<span class="tl-picker__row-label">${esc(row.label)}</span>`
      + (row.detail ? `<span class="tl-picker__row-detail" title="${esc(row.detail)}">${esc(row.detail)}</span>` : '')
      + `<span class="tl-picker__status tl-picker__status--${esc(row.status)}">${esc(meta.pill)}</span>`
      + `<select class="tl-combo-select" data-role="action"${disabled ? ' disabled' : ''} aria-label="Action for ${esc(row.label)}">${options}</select>`
      + `<input type="text" class="tl-input" data-role="rename" hidden aria-label="New label for ${esc(row.label)}" />`
      + `</div>`;
  }

  function bulkGroupHtml(status, rows) {
    const meta = statusMeta(status);
    const labelText = meta.noun(rows.length);
    if (status === 'reference_broken') {
      // Only Skip is ever valid here (Global Constraints table), so there is
      // nothing to choose — a bulk control offering one permanently-selected
      // option would just be a disabled control that does nothing new.
      return `<div class="tl-picker__section-head" data-role="bulk-group" data-status="${esc(status)}">`
        + `<span class="tl-picker__summary">${esc(labelText)} — always skipped</span>`
        + `</div>`;
    }
    const options = actionsFor(status)
      .map((a) => `<option value="${a}">${esc(BULK_ACTION_LABEL[a])}</option>`)
      .join('');
    return `<div class="tl-picker__section-head" data-role="bulk-group" data-status="${esc(status)}">`
      + `<span class="tl-picker__summary">${esc(labelText)}:</span>`
      + `<select class="tl-combo-select" data-role="bulk-action" aria-label="Bulk action for ${esc(labelText)}">${options}</select>`
      + `</div>`;
  }

  /**
   * Render the import preview into `container`.
   *
   * rows: ImportPreviewRow[] from share_import_plan — { kind, id, label,
   * detail, status, default_action }.
   *
   * Returns { decisions() } — decisions() reads the live DOM and returns the
   * ImportDecision[] array share_import_apply expects: one entry per row,
   * `{ kind, id, action, label? }` with `label` present only for `rename`.
   */
  function mount(container, rows) {
    const list = (Array.isArray(rows) ? rows : []).map(normalizeRow);
    const statusesPresent = STATUS_ORDER.filter((s) => list.some((r) => r.status === s));
    const rowsByStatus = {};
    for (const s of statusesPresent) rowsByStatus[s] = list.filter((r) => r.status === s);

    container.innerHTML =
      `<div class="tl-picker">`
      + `<input type="search" class="tl-input tl-picker__filter" data-role="filter" placeholder="Filter by label or detail…" aria-label="Filter import rows" />`
      + (statusesPresent.length
        ? statusesPresent.map((s) => bulkGroupHtml(s, rowsByStatus[s])).join('')
        : '')
      + `<div class="tl-picker__box tl-scroll" data-role="rows">`
      + list.map(rowHtml).join('')
      + `<div class="tl-picker__empty" hidden>No matches</div>`
      + `</div>`
      + `<div class="tl-picker__summary" data-role="footer"></div>`
      + `</div>`;

    const filterEl = container.querySelector('[data-role="filter"]');
    const boxEl = container.querySelector('[data-role="rows"]');
    const emptyEl = container.querySelector('.tl-picker__empty');
    const footerEl = container.querySelector('[data-role="footer"]');

    const matchesFilter = (exports.termlabExportPicker && typeof exports.termlabExportPicker.matchesFilter === 'function')
      ? exports.termlabExportPicker.matchesFilter
      // Fails open (shows everything) rather than reimplementing the
      // matching rule if the export picker module didn't load.
      : () => true;

    // Zipped by render order — list.map(rowHtml) above emitted exactly one
    // `[data-role="row"]` per entry of `list`, in the same order, so pairing
    // by index avoids any need to re-select rows by (kind, id) later (and
    // the escaping that would require, since ids are arbitrary strings).
    const rowEls = Array.prototype.slice.call(boxEl.querySelectorAll('[data-role="row"]'));
    const attachCombo = (selectEl) => {
      if (exports.tlCombo && typeof exports.tlCombo.attach === 'function') return exports.tlCombo.attach(selectEl);
      return null;
    };
    const bindings = list.map((row, i) => {
      const el = rowEls[i];
      const selectEl = el.querySelector('[data-role="action"]');
      const renameEl = el.querySelector('[data-role="rename"]');
      return { row, el, selectEl, renameEl, combo: selectEl ? attachCombo(selectEl) : null };
    });

    container.querySelectorAll('[data-role="bulk-group"] select[data-role="bulk-action"]').forEach(attachCombo);

    function existingLabelsFor(row) {
      return list.filter((r) => r.kind === row.kind).map((r) => r.label);
    }

    function updateRenameVisibility(binding) {
      if (!binding.renameEl) return;
      const isRename = binding.selectEl && binding.selectEl.value === 'rename';
      if (isRename) {
        if (binding.renameEl.hidden) {
          binding.renameEl.hidden = false;
          binding.renameEl.value = suggestRename(binding.row.label, existingLabelsFor(binding.row));
        }
      } else {
        binding.renameEl.hidden = true;
      }
    }

    function currentDecisions() {
      return bindings.map((b) => {
        const action = b.selectEl ? b.selectEl.value : 'skip';
        const decision = { kind: b.row.kind, id: b.row.id, action };
        if (action === 'rename') decision.label = b.renameEl ? String(b.renameEl.value || '').trim() : '';
        return decision;
      });
    }

    function refreshFooter() {
      if (footerEl) footerEl.textContent = summarise(currentDecisions());
    }

    function applyFilter() {
      const q = filterEl ? filterEl.value : '';
      let visible = 0;
      bindings.forEach((b) => {
        const show = matchesFilter(b.el.getAttribute('data-search'), q);
        b.el.hidden = !show;
        if (show) visible++;
      });
      if (emptyEl) emptyEl.hidden = visible !== 0;
    }

    container.addEventListener('change', (e) => {
      const target = e.target;
      if (!target || typeof target.matches !== 'function') return;

      if (target.matches('[data-role="action"]')) {
        const binding = bindings.find((b) => b.selectEl === target);
        if (binding) updateRenameVisibility(binding);
        refreshFooter();
        return;
      }

      if (target.matches('[data-role="bulk-action"]')) {
        const group = target.closest('[data-role="bulk-group"]');
        const status = group ? group.getAttribute('data-status') : null;
        const action = target.value;
        // Acts on what the filter currently shows, not on every row of this
        // status — mirrors the export picker's All/None (Global
        // Constraints), so a bulk pick after filtering can't silently touch
        // hidden rows.
        bindings.forEach((b) => {
          if (b.row.status !== status) return;
          if (b.el.hidden) return;
          if (!b.selectEl || b.selectEl.disabled) return;
          b.selectEl.value = action;
          updateRenameVisibility(b);
          if (b.combo && typeof b.combo.refresh === 'function') b.combo.refresh();
        });
        refreshFooter();
      }
    });

    if (filterEl) filterEl.addEventListener('input', applyFilter);

    bindings.forEach(updateRenameVisibility);
    applyFilter();
    refreshFooter();

    return { decisions: currentDecisions };
  }

  exports.termlabImportPreview = { mount, summarise, suggestRename };
})(window);
