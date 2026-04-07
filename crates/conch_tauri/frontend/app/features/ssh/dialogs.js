(function initConchSshDialogs(global) {
  'use strict';

  function showAddFolderDialog(deps) {
    const d = deps || {};
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.registerOverlayKeys !== 'function') return false;
    if (typeof d.invoke !== 'function') return false;
    if (typeof d.refreshAll !== 'function') return false;

    d.removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    if (typeof d.setOverlayDialogAttributes === 'function') {
      d.setOverlayDialogAttributes(overlay, 'Create new folder');
    }
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">New Folder</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Name
            <input type="text" id="fd-name" value="" placeholder="Folder name" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="fd-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="fd-create">Create</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    const nameInput = overlay.querySelector('#fd-name');
    setTimeout(() => nameInput.focus(), 50);

    let closed = false;
    const dismissDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      d.removeOverlay();
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
    const unregisterKeys = d.registerOverlayKeys(overlay, 'ssh-add-folder-dialog', (event) => {
      if (event.key === 'Escape') {
        dismissDialog();
        return true;
      }
      if (event.key === 'Enter') {
        doCreate();
        return true;
      }
      return false;
    });
    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) dismissDialog();
    });
    overlay.querySelector('#fd-cancel').addEventListener('click', dismissDialog);
    overlay.querySelector('#fd-create').addEventListener('click', doCreate);
    return true;
  }

  function showRenameFolderDialog(folder, deps) {
    const d = deps || {};
    if (!folder) return false;
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.registerOverlayKeys !== 'function') return false;
    if (typeof d.invoke !== 'function') return false;
    if (typeof d.refreshAll !== 'function') return false;

    const attr = typeof d.attr === 'function'
      ? d.attr
      : (value) => String(value == null ? '' : value);

    d.removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    if (typeof d.setOverlayDialogAttributes === 'function') {
      d.setOverlayDialogAttributes(overlay, 'Rename folder');
    }
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">Rename Folder</div>
        <div class="ssh-form-body">
          <label class="ssh-form-label">Name
            <input type="text" id="rf-name" value="${attr(folder.name)}" spellcheck="false" />
          </label>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="rf-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="rf-save">Save</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);
    const nameInput = overlay.querySelector('#rf-name');
    setTimeout(() => {
      nameInput.focus();
      nameInput.select();
    }, 50);

    let closed = false;
    const dismissDialog = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      d.removeOverlay();
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
    const unregisterKeys = d.registerOverlayKeys(overlay, 'ssh-rename-folder-dialog', (event) => {
      if (event.key === 'Escape') {
        dismissDialog();
        return true;
      }
      if (event.key === 'Enter') {
        doSave();
        return true;
      }
      return false;
    });
    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) dismissDialog();
    });
    overlay.querySelector('#rf-cancel').addEventListener('click', dismissDialog);
    overlay.querySelector('#rf-save').addEventListener('click', doSave);
    return true;
  }

  function showDeleteConfirmDialog(message, onConfirm, deps) {
    const d = deps || {};
    if (typeof d.removeOverlay !== 'function') return false;
    if (typeof d.registerOverlayKeys !== 'function') return false;

    const esc = typeof d.esc === 'function'
      ? d.esc
      : (value) => String(value == null ? '' : value);

    d.removeOverlay();
    const overlay = document.createElement('div');
    overlay.className = 'ssh-overlay';
    overlay.style.zIndex = '5000';
    if (typeof d.setOverlayDialogAttributes === 'function') {
      d.setOverlayDialogAttributes(overlay, 'Confirm delete');
    }
    overlay.innerHTML = `
      <div class="ssh-form ssh-form-small">
        <div class="ssh-form-title">Confirm Delete</div>
        <div class="ssh-form-body">
          <div class="ssh-auth-message">${esc(message)}</div>
        </div>
        <div class="ssh-form-buttons">
          <button class="ssh-form-btn" id="dc-cancel">Cancel</button>
          <button class="ssh-form-btn primary" id="dc-delete" style="background:var(--red);border-color:var(--red)">Delete</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    let closed = false;
    const dismiss = () => {
      if (closed) return;
      closed = true;
      if (typeof unregisterKeys === 'function') unregisterKeys();
      if (overlay.isConnected) overlay.remove();
    };
    const unregisterKeys = d.registerOverlayKeys(overlay, 'ssh-delete-confirm-dialog', (event) => {
      if (event.key !== 'Escape') return false;
      dismiss();
      return true;
    });
    overlay.querySelector('#dc-cancel').addEventListener('click', dismiss);
    overlay.querySelector('#dc-delete').addEventListener('click', () => {
      dismiss();
      if (typeof onConfirm === 'function') onConfirm();
    });
    overlay.addEventListener('mousedown', (event) => {
      if (event.target === overlay) dismiss();
    });
    return true;
  }

  global.conchSshDialogs = {
    showAddFolderDialog,
    showRenameFolderDialog,
    showDeleteConfirmDialog,
  };
})(window);
