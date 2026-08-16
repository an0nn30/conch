(function initTermLabSettingsPluginsSection(global) {
  'use strict';

  function createRenderer(deps) {
    const invoke = deps.invoke;
    const getPendingSettings = deps.getPendingSettings;
    const getCachedPlugins = deps.getCachedPlugins;
    const setCachedPlugins = deps.setCachedPlugins;
    const setCachedPluginMenuItems = deps.setCachedPluginMenuItems;
    const setCachedPluginSettingsSections = deps.setCachedPluginSettingsSections || (() => {});
    const refreshPluginInventory = deps.refreshPluginInventory;
    const onPluginInventoryUpdated = deps.onPluginInventoryUpdated || (() => {});
    const confirmPluginPermissions = deps.confirmPluginPermissions;
    const invalidateCommandPaletteCache = deps.invalidateCommandPaletteCache;
    const addSectionLabel = deps.addSectionLabel;
    const addDivider = deps.addDivider;
    const addRow = deps.addRow;
    const setRowTarget = deps.setRowTarget;
    const makeInput = deps.makeInput;
    const makeCheckbox = deps.makeCheckbox;

    function refreshTitlebar() {
      if (global.titlebar && typeof global.titlebar.refresh === 'function') {
        global.titlebar.refresh().catch(() => {});
      }
    }

    function toastInfo(title, body) {
      if (global.toast && typeof global.toast.info === 'function') {
        global.toast.info(title, body);
      }
    }

    function toastSuccess(title, body) {
      if (global.toast && typeof global.toast.success === 'function') {
        global.toast.success(title, body);
      }
    }

    function toastError(title, body) {
      if (global.toast && typeof global.toast.error === 'function') {
        global.toast.error(title, body);
      }
    }

    function renderPlugins(container) {
      const pendingSettings = getPendingSettings();
      if (!pendingSettings || !pendingSettings.termlab || !pendingSettings.termlab.plugins) return;

      const heading = document.createElement('h3');
      heading.textContent = 'Plugins';
      container.appendChild(heading);

      addSectionLabel(container, 'System');
      const pluginsCheckbox = makeCheckbox(
        pendingSettings.termlab.plugins.enabled,
        (value) => { pendingSettings.termlab.plugins.enabled = value; }
      );
      setRowTarget(
        addRow(container, 'Enable Plugins', 'Master toggle for plugin system', pluginsCheckbox),
        'plugins:enabled'
      );

      addDivider(container);

      addSectionLabel(container, 'Plugin Types');
      const pluginTypesAnchor = document.createElement('div');
      pluginTypesAnchor.dataset.settingId = 'plugins:types';
      container.appendChild(pluginTypesAnchor);

      const luaCheckbox = makeCheckbox(
        pendingSettings.termlab.plugins.lua,
        (value) => { pendingSettings.termlab.plugins.lua = value; }
      );
      addRow(container, 'Lua Plugins', null, luaCheckbox);

      const javaCheckbox = makeCheckbox(
        pendingSettings.termlab.plugins.java,
        (value) => { pendingSettings.termlab.plugins.java = value; }
      );
      addRow(container, 'Java Plugins', 'Disabling avoids JVM startup overhead', javaCheckbox);

      addDivider(container);

      addSectionLabel(container, 'Extra Search Paths');
      const searchPathsHint = document.createElement('div');
      searchPathsHint.dataset.settingId = 'plugins:search-paths';
      searchPathsHint.className = 'tl-settings__row-desc';
      searchPathsHint.style.marginBottom = '8px';
      searchPathsHint.textContent = 'Built-in defaults always include ~/.config/termlab/plugins. Add extra directories here.';
      container.appendChild(searchPathsHint);

      const pathsContainer = document.createElement('div');
      container.appendChild(pathsContainer);

      function renderSearchPaths() {
        pathsContainer.innerHTML = '';
        const paths = pendingSettings.termlab.plugins.search_paths || [];

        for (let i = 0; i < paths.length; i++) {
          const row = document.createElement('div');
          row.style.cssText = 'display:flex; align-items:center; gap:6px; margin-bottom:4px;';

          const pathInput = makeInput('text', paths[i], { style: 'flex:1;' });
          pathInput.addEventListener('input', () => {
            pendingSettings.termlab.plugins.search_paths[i] = pathInput.value;
          });
          row.appendChild(pathInput);

          const removeBtn = document.createElement('button');
          removeBtn.className = 'ssh-form-btn settings-env-remove';
          removeBtn.textContent = 'X';
          removeBtn.addEventListener('click', () => {
            pendingSettings.termlab.plugins.search_paths.splice(i, 1);
            renderSearchPaths();
          });
          row.appendChild(removeBtn);

          pathsContainer.appendChild(row);
        }

        const addBtn = document.createElement('button');
        addBtn.className = 'ssh-form-btn settings-env-add';
        addBtn.textContent = '+ Add Path';
        addBtn.addEventListener('click', () => {
          if (!pendingSettings.termlab.plugins.search_paths) {
            pendingSettings.termlab.plugins.search_paths = [];
          }
          pendingSettings.termlab.plugins.search_paths.push('');
          renderSearchPaths();
        });
        pathsContainer.appendChild(addBtn);
      }

      renderSearchPaths();

      addDivider(container);

      // Uses addSectionLabel's optional trailingEl (Task 1 review fix
      // round 1, finding 2) instead of the old hand-built
      // .settings-installed-header/.settings-section-label pair, so this
      // header gets the same label-beside-rule treatment as every other
      // section header in the new shell — addDivider() above is a no-op
      // (see its own comment), so without this the separator here would
      // have silently vanished and the header would have kept the old,
      // pre-migration look.
      const rescanLabel = document.createElement('span');
      rescanLabel.textContent = 'Rescan';
      rescanLabel.setAttribute('role', 'button');
      rescanLabel.setAttribute('tabindex', '0');
      rescanLabel.className = 'tl-settings__link';

      const handleRescan = async () => {
        rescanLabel.style.pointerEvents = 'none';
        rescanLabel.style.opacity = '0.6';
        try {
          const inventory = await refreshPluginInventory();
          setCachedPlugins(inventory.plugins);
          setCachedPluginMenuItems(inventory.pluginMenuItems);
          setCachedPluginSettingsSections(inventory.pluginSettingsSections);
          invalidateCommandPaletteCache('plugin-rescan');
          refreshTitlebar();
          onPluginInventoryUpdated();
        } catch (error) {
          toastError('Plugin Scan Failed', String(error));
        }
        rescanLabel.style.pointerEvents = 'auto';
        rescanLabel.style.opacity = '1';
        renderPluginList();
      };

      rescanLabel.addEventListener('click', handleRescan);
      rescanLabel.addEventListener('keydown', (event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        handleRescan();
      });
      const installedHeader = addSectionLabel(container, 'Installed Plugins', rescanLabel);
      installedHeader.dataset.settingId = 'plugins:installed';

      const pluginListContainer = document.createElement('div');
      container.appendChild(pluginListContainer);

      function renderPluginList() {
        pluginListContainer.innerHTML = '';
        const cachedPlugins = getCachedPlugins();
        if (!cachedPlugins || cachedPlugins.length === 0) {
          const empty = document.createElement('div');
          empty.className = 'settings-plugin-empty';
          empty.textContent = 'No plugins found in search paths';
          pluginListContainer.appendChild(empty);
          return;
        }

        for (const plugin of cachedPlugins) {
          const row = document.createElement('div');
          row.className = 'settings-plugin-row';

          const left = document.createElement('div');
          left.className = 'settings-plugin-main';

          const badge = document.createElement('span');
          const pluginType = (plugin.plugin_type || '').toLowerCase();
          badge.className = 'tl-settings__plugin-badge' + (pluginType === 'lua' ? ' tl-settings__plugin-badge--lua' : ' tl-settings__plugin-badge--java');
          badge.textContent = pluginType;
          left.appendChild(badge);

          const info = document.createElement('div');
          info.className = 'settings-plugin-info';
          const nameEl = document.createElement('div');
          nameEl.className = 'settings-plugin-name';
          nameEl.textContent = plugin.name;
          info.appendChild(nameEl);
          const meta = document.createElement('div');
          meta.className = 'settings-plugin-meta';
          meta.textContent = (plugin.version || '') + ' \u2014 ' + (plugin.path || '');
          info.appendChild(meta);
          left.appendChild(info);
          row.appendChild(left);

          // Wrapped in the shared .tl-check component (rather than left as a
          // bare, browser-unstyled checkbox) so it matches the styled
          // checkboxes immediately above it on this same page (Enable
          // Plugins / Lua / Java) instead of standing out as the one
          // native-looking control on the Plugins section.
          const toggleLabel = document.createElement('label');
          toggleLabel.className = 'tl-check settings-plugin-toggle';
          const toggle = document.createElement('input');
          toggle.type = 'checkbox';
          toggle.checked = !!plugin.loaded;
          toggle.setAttribute('aria-label', (plugin.loaded ? 'Disable ' : 'Enable ') + plugin.name);
          toggle.addEventListener('change', async () => {
            const nextLoaded = toggle.checked;
            toggle.disabled = true;
            try {
              const result = await global.termlabSettingsFeatureDataService.setPluginLoadedState(invoke, plugin, nextLoaded, {
                confirmPermissions: (pluginName, permissions) => confirmPluginPermissions(pluginName, permissions),
              });

              if (result && result.status === 'cancelled') {
                toggle.checked = false;
                return;
              }
              if (result && result.status === 'disabled') {
                toastInfo('Plugin Disabled', plugin.name);
              } else if (result && result.status === 'enabled') {
                toastSuccess('Plugin Enabled', plugin.name);
              }

              const inventory = await refreshPluginInventory();
              setCachedPlugins(inventory.plugins);
              setCachedPluginMenuItems(inventory.pluginMenuItems);
              setCachedPluginSettingsSections(inventory.pluginSettingsSections);
              invalidateCommandPaletteCache('plugin-toggle');
              refreshTitlebar();
              onPluginInventoryUpdated();
            } catch (error) {
              toggle.checked = !!plugin.loaded;
              toastError('Plugin Action Failed', String(error));
            }
            toggle.disabled = false;
            renderPluginList();
          });
          toggleLabel.appendChild(toggle);
          row.appendChild(toggleLabel);
          pluginListContainer.appendChild(row);
        }
      }

      renderPluginList();
    }

    return {
      renderPlugins,
    };
  }

  global.termlabSettingsPluginsSection = {
    createRenderer,
  };
})(window);
