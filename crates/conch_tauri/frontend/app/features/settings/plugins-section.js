(function initConchSettingsPluginsSection(global) {
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
    const makeSwitch = deps.makeSwitch;

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
      if (!pendingSettings || !pendingSettings.conch || !pendingSettings.conch.plugins) return;

      const heading = document.createElement('h3');
      heading.textContent = 'Plugins';
      container.appendChild(heading);

      addSectionLabel(container, 'System');
      const pluginsSwitch = makeSwitch(
        pendingSettings.conch.plugins.enabled,
        (value) => { pendingSettings.conch.plugins.enabled = value; }
      );
      setRowTarget(
        addRow(container, 'Enable Plugins', 'Master toggle for plugin system', pluginsSwitch),
        'plugins:enabled'
      );

      addDivider(container);

      addSectionLabel(container, 'Plugin Types');
      const pluginTypesAnchor = document.createElement('div');
      pluginTypesAnchor.dataset.settingId = 'plugins:types';
      container.appendChild(pluginTypesAnchor);

      const luaSwitch = makeSwitch(
        pendingSettings.conch.plugins.lua,
        (value) => { pendingSettings.conch.plugins.lua = value; }
      );
      addRow(container, 'Lua Plugins', null, luaSwitch);

      const javaSwitch = makeSwitch(
        pendingSettings.conch.plugins.java,
        (value) => { pendingSettings.conch.plugins.java = value; }
      );
      addRow(container, 'Java Plugins', 'Disabling avoids JVM startup overhead', javaSwitch);

      addDivider(container);

      addSectionLabel(container, 'Extra Search Paths');
      const searchPathsHint = document.createElement('div');
      searchPathsHint.dataset.settingId = 'plugins:search-paths';
      searchPathsHint.className = 'settings-row-desc';
      searchPathsHint.style.marginBottom = '8px';
      searchPathsHint.textContent = 'Built-in defaults always include ~/.config/conch/plugins. Add extra directories here.';
      container.appendChild(searchPathsHint);

      const pathsContainer = document.createElement('div');
      container.appendChild(pathsContainer);

      function renderSearchPaths() {
        pathsContainer.innerHTML = '';
        const paths = pendingSettings.conch.plugins.search_paths || [];

        for (let i = 0; i < paths.length; i++) {
          const row = document.createElement('div');
          row.style.cssText = 'display:flex; align-items:center; gap:6px; margin-bottom:4px;';

          const pathInput = makeInput('text', paths[i], { style: 'flex:1;' });
          pathInput.addEventListener('input', () => {
            pendingSettings.conch.plugins.search_paths[i] = pathInput.value;
          });
          row.appendChild(pathInput);

          const removeBtn = document.createElement('button');
          removeBtn.className = 'ssh-form-btn settings-env-remove';
          removeBtn.textContent = 'X';
          removeBtn.addEventListener('click', () => {
            pendingSettings.conch.plugins.search_paths.splice(i, 1);
            renderSearchPaths();
          });
          row.appendChild(removeBtn);

          pathsContainer.appendChild(row);
        }

        const addBtn = document.createElement('button');
        addBtn.className = 'ssh-form-btn settings-env-add';
        addBtn.textContent = '+ Add Path';
        addBtn.addEventListener('click', () => {
          if (!pendingSettings.conch.plugins.search_paths) {
            pendingSettings.conch.plugins.search_paths = [];
          }
          pendingSettings.conch.plugins.search_paths.push('');
          renderSearchPaths();
        });
        pathsContainer.appendChild(addBtn);
      }

      renderSearchPaths();

      addDivider(container);

      const installedHeader = document.createElement('div');
      installedHeader.dataset.settingId = 'plugins:installed';
      installedHeader.className = 'settings-installed-header';
      const installedLabel = document.createElement('div');
      installedLabel.className = 'settings-section-label';
      installedLabel.textContent = 'Installed Plugins';
      installedHeader.appendChild(installedLabel);

      const rescanLabel = document.createElement('span');
      rescanLabel.textContent = 'Rescan';
      rescanLabel.setAttribute('role', 'button');
      rescanLabel.setAttribute('tabindex', '0');
      rescanLabel.className = 'settings-rescan-action';

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
      installedHeader.appendChild(rescanLabel);
      container.appendChild(installedHeader);

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
          badge.className = 'settings-plugin-badge' + (pluginType === 'lua' ? ' lua' : ' java');
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

          const toggle = document.createElement('input');
          toggle.type = 'checkbox';
          toggle.checked = !!plugin.loaded;
          toggle.className = 'settings-plugin-toggle';
          toggle.setAttribute('aria-label', (plugin.loaded ? 'Disable ' : 'Enable ') + plugin.name);
          toggle.addEventListener('change', async () => {
            const nextLoaded = toggle.checked;
            toggle.disabled = true;
            try {
              let result = null;
              if (global.conchSettingsFeatureDataService && typeof global.conchSettingsFeatureDataService.setPluginLoadedState === 'function') {
                result = await global.conchSettingsFeatureDataService.setPluginLoadedState(invoke, plugin, nextLoaded, {
                  confirmPermissions: (pluginName, permissions) => confirmPluginPermissions(pluginName, permissions),
                });
              } else if (!nextLoaded) {
                await invoke('disable_plugin', { name: plugin.name, source: plugin.source });
                await invoke('rebuild_menu').catch(() => {});
                result = { status: 'disabled' };
              } else {
                await invoke('enable_plugin', { name: plugin.name, source: plugin.source, path: plugin.path });
                await invoke('rebuild_menu').catch(() => {});
                result = { status: 'enabled' };
              }

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
          row.appendChild(toggle);
          pluginListContainer.appendChild(row);
        }
      }

      renderPluginList();
    }

    return {
      renderPlugins,
    };
  }

  global.conchSettingsPluginsSection = {
    createRenderer,
  };
})(window);
