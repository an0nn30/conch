(function initConchToolWindowRuntime(global) {
  function create(deps) {
    const invoke = deps.invoke;
    const listenOnCurrentWindow = deps.listenOnCurrentWindow;
    const debouncedFitAndResize = deps.debouncedFitAndResize;
    const getCurrentTab = deps.getCurrentTab;
    const getCurrentPane = deps.getCurrentPane;
    const createSshTab = deps.createSshTab;
    const openPluginDockedViewFromRequest = deps.openPluginDockedViewFromRequest;
    const setFocusedPane = deps.setFocusedPane;
    const closePane = deps.closePane;
    const getPluginViewPaneById = deps.getPluginViewPaneById;

    async function init() {
      const bottomPanelEl = document.getElementById('bottom-panel');
      const bottomResizeEl = document.getElementById('bottom-panel-resize');

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
              ssh_panel_visible: twm.isVisible('ssh-sessions'),
              files_panel_width: widths.left,
              files_panel_visible: twm.isVisible('file-explorer'),
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
          const layoutData = await invoke('get_saved_layout');
          if (layoutData.files_panel_width > 100) {
            global.toolWindowManager.setSidebarWidth('left', layoutData.files_panel_width);
          }
          if (layoutData.ssh_panel_width > 100) {
            global.toolWindowManager.setSidebarWidth('right', layoutData.ssh_panel_width);
          }
          if (layoutData.tool_window_zones && Object.keys(layoutData.tool_window_zones).length > 0) {
            global.toolWindowManager.setPersistedZones(layoutData.tool_window_zones);
          }
          if (layoutData.left_split_ratio > 0 && layoutData.left_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('left', layoutData.left_split_ratio);
          }
          if (layoutData.right_split_ratio > 0 && layoutData.right_split_ratio < 1) {
            global.toolWindowManager.setSplitRatio('right', layoutData.right_split_ratio);
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
          listen: listenOnCurrentWindow,
          writeToActivePty: (data) => {
            const pane = getCurrentPane();
            if (!pane || !pane.spawned) return;
            const cmd = pane.type === 'ssh' ? 'ssh_write' : 'write_to_pty';
            invoke(cmd, { paneId: pane.paneId, data }).catch(() => {});
          },
        });

        listenOnCurrentWindow('plugin-panel-registered', async (event) => {
          const { handle, plugin, name, location } = event.payload;
          if (global.titlebar && typeof global.titlebar.refresh === 'function') {
            global.titlebar.refresh().catch(() => {});
          }
          if (location === 'bottom') return;

          const zoneMap = { left: 'left-top', right: 'right-top' };
          const defaultZone = zoneMap[location] || 'right-bottom';
          const twmId = 'plugin:' + plugin;
          if (global.toolWindowManager) {
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
          }
        });

        listenOnCurrentWindow('plugin-panels-removed', (event) => {
          if (global.titlebar && typeof global.titlebar.refresh === 'function') {
            global.titlebar.refresh().catch(() => {});
          }
          const { plugin, handles } = event.payload;
          if (global.toolWindowManager) {
            global.toolWindowManager.unregister('plugin:' + plugin);
          }
          for (const handle of handles) {
            const container = document.querySelector(`[data-plugin-handle="${handle}"]`);
            if (container) container.remove();
          }
        });

        listenOnCurrentWindow('plugin-view-open-requested', (event) => {
          openPluginDockedViewFromRequest(event.payload).catch((error) => {
            console.error('Failed to open plugin docked view:', error);
          });
        });

        listenOnCurrentWindow('plugin-view-focus-requested', (event) => {
          const viewId = event && event.payload ? event.payload.view_id : null;
          const map = getPluginViewPaneById();
          if (!viewId || !map.has(viewId)) return;
          setFocusedPane(map.get(viewId));
        });

        listenOnCurrentWindow('plugin-view-close-requested', (event) => {
          const viewId = event && event.payload ? event.payload.view_id : null;
          const map = getPluginViewPaneById();
          if (!viewId || !map.has(viewId)) return;
          closePane(map.get(viewId));
        });

        listenOnCurrentWindow('plugin-views-removed', (event) => {
          if (global.titlebar && typeof global.titlebar.refresh === 'function') {
            global.titlebar.refresh().catch(() => {});
          }
          const viewIds = (event && event.payload && event.payload.view_ids) || [];
          const map = getPluginViewPaneById();
          for (const viewId of viewIds) {
            if (!map.has(viewId)) continue;
            closePane(map.get(viewId));
          }
        });
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
