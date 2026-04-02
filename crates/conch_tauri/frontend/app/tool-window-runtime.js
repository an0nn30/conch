(function initConchToolWindowRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listen = deps.listen;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const debouncedFitAndResize = deps.debouncedFitAndResize;
    const getCurrentTab = deps.getCurrentTab;
    const getCurrentPane = deps.getCurrentPane;
    const createSshTab = deps.createSshTab;
    const registeredPluginToolWindows = new Set();

    async function init() {
      const bottomPanelEl = document.getElementById('bottom-panel');
      const bottomResizeEl = document.getElementById('bottom-panel-resize');
      let initialLayoutData = null;
      const refreshShortcutFallbacks = () => {
        if (typeof global.__conchRefreshKeyboardShortcutFallbacks === 'function') {
          global.__conchRefreshKeyboardShortcutFallbacks().catch(() => {});
        }
      };

      let windowResaveSaveTimer = null;
      function debouncedSaveLayout() {
        if (windowResaveSaveTimer) clearTimeout(windowResaveSaveTimer);
        windowResaveSaveTimer = setTimeout(() => {
          const twm = global.toolWindowManager;
          if (!twm) return;
          const widths = twm.getSidebarWidths();
          invoke('save_window_layout', {
            layout: {
              ssh_panel_width: widths.right,
              ssh_panel_visible: twm.isPanelVisible('right'),
              files_panel_width: widths.left,
              files_panel_visible: twm.isPanelVisible('left'),
              bottom_panel_visible: !bottomPanelEl.classList.contains('hidden'),
              bottom_panel_height: bottomPanelEl.offsetHeight,
              tool_window_zones: twm.getZoneAssignments(),
              split_ratios: twm.getSplitRatios(),
            },
          }).catch(() => {});
        }, 500);
      }

      if (global.toolWindowManager) {
        global.toolWindowManager.init({
          fitActiveTab: debouncedFitAndResize,
          saveLayout: debouncedSaveLayout,
        });

        try {
          initialLayoutData = await invoke('get_saved_layout');
          if (initialLayoutData.files_panel_width > 100) {
            global.toolWindowManager.setSidebarWidth('left', initialLayoutData.files_panel_width);
          }
          if (initialLayoutData.ssh_panel_width > 100) {
            global.toolWindowManager.setSidebarWidth('right', initialLayoutData.ssh_panel_width);
          }
          if (initialLayoutData.tool_window_zones && Object.keys(initialLayoutData.tool_window_zones).length > 0) {
            global.toolWindowManager.setPersistedZones(initialLayoutData.tool_window_zones);
          }
          if (initialLayoutData.left_split_ratio > 0 && initialLayoutData.left_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('left', initialLayoutData.left_split_ratio);
          }
          if (initialLayoutData.right_split_ratio > 0 && initialLayoutData.right_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('right', initialLayoutData.right_split_ratio);
          }
        } catch (_) {}

        global.toolWindowManager.register('file-explorer', {
          title: 'Files',
          type: 'built-in',
          defaultZone: 'left-top',
          renderFn: (container) => {
            const panelEl = document.createElement('div');
            panelEl.id = 'files-panel';
            container.appendChild(panelEl);
            if (global.filesPanel) {
              global.filesPanel.init({
                invoke,
                listen: listenOnCurrentWindow,
                panelEl,
                panelWrapEl: document.getElementById('left-sidebar'),
                resizeHandleEl: null,
                fitActiveTab: debouncedFitAndResize,
                getActiveTab: () => getCurrentTab(),
              });
            }
          },
        });

        global.toolWindowManager.register('ssh-sessions', {
          title: 'Sessions',
          type: 'built-in',
          defaultZone: 'right-top',
          renderFn: (container) => {
            const panelEl = document.createElement('div');
            panelEl.id = 'ssh-panel';
            container.appendChild(panelEl);
            if (global.sshPanel) {
              global.sshPanel.init({
                invoke,
                listen: listenOnCurrentWindow,
                createSshTab,
                panelEl,
                panelWrapEl: document.getElementById('right-sidebar'),
                resizeHandleEl: null,
                fitActiveTab: debouncedFitAndResize,
                refocusTerminal: () => {
                  const pane = getCurrentPane();
                  if (pane && pane.term) pane.term.focus();
                },
              });
            }
          },
        });
        if (initialLayoutData) {
          global.toolWindowManager.setPanelVisibility('left', initialLayoutData.files_panel_visible !== false, { save: false });
          global.toolWindowManager.setPanelVisibility('right', initialLayoutData.ssh_panel_visible !== false, { save: false });
        }
        refreshShortcutFallbacks();
      }

      global.addEventListener('resize', debouncedSaveLayout);

      {
        let dragging = false;
        let startY = 0;
        let startHeight = 0;
        bottomResizeEl.addEventListener('dragstart', (event) => event.preventDefault());
        bottomResizeEl.style.touchAction = 'none';
        bottomResizeEl.addEventListener('pointerdown', (event) => {
          event.preventDefault();
          bottomResizeEl.setPointerCapture(event.pointerId);
          dragging = true;
          startY = event.clientY;
          startHeight = bottomPanelEl.offsetHeight;
          bottomResizeEl.classList.add('dragging');
          document.body.style.cursor = 'row-resize';
          document.body.style.userSelect = 'none';
        });
        bottomResizeEl.addEventListener('pointermove', (event) => {
          if (!dragging) return;
          const delta = startY - event.clientY;
          const newHeight = Math.max(40, Math.min(300, startHeight + delta));
          bottomPanelEl.style.height = newHeight + 'px';
          debouncedFitAndResize();
        });
        bottomResizeEl.addEventListener('pointerup', (event) => {
          if (!dragging) return;
          bottomResizeEl.releasePointerCapture(event.pointerId);
          dragging = false;
          bottomResizeEl.classList.remove('dragging');
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
          debouncedSaveLayout();
        });
      }

      if (global.vault) {
        global.vault.init({ invoke, listen: listenOnCurrentWindow });
      }

      if (global.keygen) {
        global.keygen.init({ invoke });
        listenOnCurrentWindow('keygen-open', () => global.keygen.showKeygenDialog());
      }

      if (global.tunnelManager) {
        global.tunnelManager.init({
          invoke,
          listen: listenOnCurrentWindow,
          getServerData: () => (
            global.sshPanel ? global.sshPanel.getServerData() : { folders: [], ungrouped: [], ssh_config: [] }
          ),
        });
      }

      if (global.settings) {
        global.settings.init({ invoke, listen: listenOnCurrentWindow });
      }

      listenOnCurrentWindow('settings-restart-required', () => {
        if (global.toast) global.toast.warn('Restart Required', 'Some changes require a restart to take effect.');
      });

      if (global.pluginWidgets) {
        global.pluginWidgets.init({
          invoke,
          listen,
          createTab: () => deps.createTab(),
          writeToActivePty: (data) => {
            const pane = getCurrentPane();
            if (!pane || !pane.spawned) return;
            const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
            invoke(cmd, { paneId: pane.paneId, data }).catch(() => {});
          },
        });

        const registerPluginToolWindow = async (panelInfo) => {
          const { handle, plugin, name, location } = panelInfo || {};
          if (global.titlebar && typeof global.titlebar.refresh === 'function') {
            global.titlebar.refresh().catch(() => {});
          }
          if (location === 'bottom') return;

          const zoneMap = { left: 'left-top', right: 'right-top' };
          const defaultZone = zoneMap[location] || 'right-bottom';
          const twmId = 'plugin:' + plugin;
          if (global.toolWindowManager && plugin && !registeredPluginToolWindows.has(twmId)) {
            registeredPluginToolWindows.add(twmId);
            global.toolWindowManager.register(twmId, {
              title: name || plugin,
              type: 'plugin',
              defaultZone,
              renderFn: async (container) => {
                const inner = document.createElement('div');
                inner.className = 'plugin-panel-content';
                inner.dataset.pluginHandle = handle;
                inner.dataset.pluginName = plugin;
                container.appendChild(inner);
                try {
                  const result = await invoke('request_plugin_render', { pluginName: plugin });
                  if (result) global.pluginWidgets.renderWidgets(inner, result, plugin);
                } catch (error) {
                  console.error('Initial plugin render failed:', error);
                }
              },
            });
            refreshShortcutFallbacks();
          }
        };

        listen('plugin-panel-registered', async (event) => {
          await registerPluginToolWindow(event.payload);
        });

        listen('plugin-panels-removed', (event) => {
          if (global.titlebar && typeof global.titlebar.refresh === 'function') {
            global.titlebar.refresh().catch(() => {});
          }
          const { plugin, handles } = event.payload;
          if (global.toolWindowManager) {
            registeredPluginToolWindows.delete('plugin:' + plugin);
            global.toolWindowManager.unregister('plugin:' + plugin);
            refreshShortcutFallbacks();
          }
          for (const handle of handles) {
            const container = document.querySelector(`[data-plugin-handle="${handle}"]`);
            if (container) container.remove();
          }
        });

        invoke('get_plugin_panels').then(async (panels) => {
          if (!Array.isArray(panels)) return;
          for (const panel of panels) {
            await registerPluginToolWindow({
              handle: panel.handle,
              plugin: panel.plugin_name,
              name: panel.panel_name,
              location: panel.location,
            });
          }
        }).catch(() => {});

      }

      return {
        debouncedSaveLayout,
      };
    }

    return {
      init,
    };
  }

  global.conchToolWindowRuntime = {
    create,
  };
})(window);
