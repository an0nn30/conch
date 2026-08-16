(function initTermLabSshDialogs(global) {
  'use strict';

  function showAddFolderDialog(deps) {
    const d = deps || {};
    if (typeof d.invoke !== 'function') return false;
    if (typeof d.refreshAll !== 'function') return false;
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    let handle = null;
    let nameInput = null;
    let closed = false;
    const dismissDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    const doCreate = () => {
      const name = nameInput.value.trim();
      if (!name) {
        nameInput.focus();
        return;
      }
      dismissDialog();
      d.invoke('remote_add_folder', { name })
        .then(() => d.refreshAll())
        .catch((error) => {
          if (d.toast && typeof d.toast.error === 'function') {
            d.toast.error('Folder Error', String(error));
          }
        });
    };

    handle = global.tlDialog.open({
      title: 'New Folder',
      ariaLabel: 'Create new folder',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Name</span>
            <input type="text" class="tl-input" id="fd-name" value="" placeholder="Folder name" spellcheck="false" />
          </div>
        `;
        nameInput = bodyEl.querySelector('#fd-name');
      },
      buttons: [
        { label: 'Cancel', onSelect: dismissDialog },
        { label: 'Create', primary: true, onSelect: doCreate },
      ],
      onClose: dismissDialog,
      // The old handler mapped Enter to "Create" globally (document-level
      // capture) regardless of which element inside the dialog had focus —
      // including a footer button, where a plain body-scoped bubble listener
      // would miss it (the footer isn't a descendant of the body) and the
      // button's native Enter-triggers-click would fire instead. Listening
      // on the fully-built panel (available here, after the footer is
      // attached) reproduces the old "always wins" behavior.
      onOpen: (panelEl) => {
        panelEl.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          doCreate();
        });
      },
    });
    if (nameInput) nameInput.focus();
    return handle;
  }

  function showRenameFolderDialog(folder, deps) {
    const d = deps || {};
    if (!folder) return false;
    if (typeof d.invoke !== 'function') return false;
    if (typeof d.refreshAll !== 'function') return false;
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);

    let handle = null;
    let nameInput = null;
    let closed = false;
    const dismissDialog = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };
    const doSave = () => {
      const name = nameInput.value.trim();
      if (!name) {
        nameInput.focus();
        return;
      }
      dismissDialog();
      d.invoke('remote_rename_folder', { folderId: folder.id, newName: name })
        .then(() => d.refreshAll())
        .catch((error) => {
          if (d.toast && typeof d.toast.error === 'function') {
            d.toast.error('Error', String(error));
          }
        });
    };

    handle = global.tlDialog.open({
      title: 'Rename Folder',
      ariaLabel: 'Rename folder',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `
          <div class="tl-field">
            <span class="tl-field__label">Name</span>
            <input type="text" class="tl-input" id="rf-name" value="${attr(folder.name)}" spellcheck="false" />
          </div>
        `;
        nameInput = bodyEl.querySelector('#rf-name');
      },
      buttons: [
        { label: 'Cancel', onSelect: dismissDialog },
        { label: 'Save', primary: true, onSelect: doSave },
      ],
      onClose: dismissDialog,
      // See showAddFolderDialog above: Enter must win regardless of which
      // element inside the dialog has focus, including a footer button.
      onOpen: (panelEl) => {
        panelEl.addEventListener('keydown', (event) => {
          if (event.key !== 'Enter') return;
          event.preventDefault();
          doSave();
        });
      },
    });
    if (nameInput) {
      nameInput.focus();
      nameInput.select();
    }
    return handle;
  }

  function showDeleteConfirmDialog(message, onConfirm, deps) {
    const d = deps || {};
    const esc = typeof d.esc === 'function'
      ? d.esc
      : (value) => String(value == null ? '' : value);
    if (!global.tlDialog || typeof global.tlDialog.open !== 'function') return false;

    let handle = null;
    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (handle) handle.close();
    };

    handle = global.tlDialog.open({
      title: 'Confirm Delete',
      ariaLabel: 'Confirm delete',
      size: 'sm',
      body: (bodyEl) => {
        bodyEl.innerHTML = `<div class="ssh-auth-message">${esc(message)}</div>`;
      },
      buttons: [
        { label: 'Cancel', onSelect: dismiss },
        { label: 'Delete', primary: true, danger: true, onSelect: () => {
          dismiss();
          if (typeof onConfirm === 'function') onConfirm();
        } },
      ],
      onClose: dismiss,
    });
    return handle;
  }

  global.termlabSshDialogs = {
    showAddFolderDialog,
    showRenameFolderDialog,
    showDeleteConfirmDialog,
  };
})(window);
