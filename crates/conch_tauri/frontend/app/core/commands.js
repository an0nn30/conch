// Auto-maintained command name constants.
// Keep in sync with #[tauri::command] functions in Rust.
//
// Generated from crates/conch_tauri/src/ — update when adding new commands.

export const COMMANDS = {
  // PTY
  SPAWN_SHELL: 'spawn_shell',
  WRITE_TO_PTY: 'write_to_pty',
  RESIZE_PTY: 'resize_pty',
  CLOSE_PTY: 'close_pty',

  // General
  GET_APP_CONFIG: 'get_app_config',
  GET_ABOUT_INFO: 'get_about_info',
  GET_HOME_DIR: 'get_home_dir',
  OPEN_DEVTOOLS: 'open_devtools',
  GET_THEME_COLORS: 'get_theme_colors',
  GET_TERMINAL_CONFIG: 'get_terminal_config',
  GET_KEYBOARD_SHORTCUTS: 'get_keyboard_shortcuts',
  APP_READY: 'app_ready',
  GET_SAVED_LAYOUT: 'get_saved_layout',
  SAVE_WINDOW_LAYOUT: 'save_window_layout',
  SET_ZOOM_LEVEL: 'set_zoom_level',
  GET_ZOOM_LEVEL: 'get_zoom_level',
  CURRENT_WINDOW_LABEL: 'current_window_label',
  SET_ACTIVE_PANE: 'set_active_pane',
  REBUILD_MENU: 'rebuild_menu',

  // Windows
  OPEN_NEW_WINDOW: 'open_new_window',

  // Settings
  GET_ALL_SETTINGS: 'get_all_settings',
  LIST_THEMES: 'list_themes',
  PREVIEW_THEME_COLORS: 'preview_theme_colors',
  SAVE_SETTINGS: 'save_settings',
  LIST_SYSTEM_FONTS: 'list_system_fonts',

  // SSH
  SSH_CONNECT: 'ssh_connect',
  SSH_QUICK_CONNECT: 'ssh_quick_connect',
  SSH_WRITE: 'ssh_write',
  SSH_RESIZE: 'ssh_resize',
  SSH_DISCONNECT: 'ssh_disconnect',
  SSH_OPEN_CHANNEL: 'ssh_open_channel',

  // Auth
  AUTH_RESPOND_HOST_KEY: 'auth_respond_host_key',
  AUTH_RESPOND_PASSWORD: 'auth_respond_password',

  // Server management
  REMOTE_GET_SERVERS: 'remote_get_servers',
  REMOTE_SAVE_SERVER: 'remote_save_server',
  REMOTE_DELETE_SERVER: 'remote_delete_server',
  REMOTE_ADD_FOLDER: 'remote_add_folder',
  REMOTE_DELETE_FOLDER: 'remote_delete_folder',
  REMOTE_IMPORT_SSH_CONFIG: 'remote_import_ssh_config',
  REMOTE_RENAME_FOLDER: 'remote_rename_folder',
  REMOTE_SET_FOLDER_EXPANDED: 'remote_set_folder_expanded',
  REMOTE_MOVE_SERVER: 'remote_move_server',
  REMOTE_EXPORT: 'remote_export',
  REMOTE_IMPORT: 'remote_import',
  REMOTE_DUPLICATE_SERVER: 'remote_duplicate_server',
  REMOTE_GET_SESSIONS: 'remote_get_sessions',

  // SFTP / Local FS
  SFTP_LIST_DIR: 'sftp_list_dir',
  SFTP_STAT: 'sftp_stat',
  SFTP_READ_FILE: 'sftp_read_file',
  SFTP_WRITE_FILE: 'sftp_write_file',
  SFTP_MKDIR: 'sftp_mkdir',
  SFTP_RENAME: 'sftp_rename',
  SFTP_REMOVE: 'sftp_remove',
  SFTP_REALPATH: 'sftp_realpath',
  LOCAL_LIST_DIR: 'local_list_dir',
  LOCAL_STAT: 'local_stat',
  LOCAL_MKDIR: 'local_mkdir',
  LOCAL_RENAME: 'local_rename',
  LOCAL_REMOVE: 'local_remove',

  // Transfers
  TRANSFER_DOWNLOAD: 'transfer_download',
  TRANSFER_UPLOAD: 'transfer_upload',
  TRANSFER_CANCEL: 'transfer_cancel',

  // Tunnels
  TUNNEL_START: 'tunnel_start',
  TUNNEL_STOP: 'tunnel_stop',
  TUNNEL_SAVE: 'tunnel_save',
  TUNNEL_DELETE: 'tunnel_delete',
  TUNNEL_GET_ALL: 'tunnel_get_all',

  // Vault
  VAULT_STATUS: 'vault_status',
  VAULT_CREATE: 'vault_create',
  VAULT_UNLOCK: 'vault_unlock',
  VAULT_LOCK: 'vault_lock',
  VAULT_LIST_ACCOUNTS: 'vault_list_accounts',
  VAULT_GET_ACCOUNT: 'vault_get_account',
  VAULT_ADD_ACCOUNT: 'vault_add_account',
  VAULT_UPDATE_ACCOUNT: 'vault_update_account',
  VAULT_DELETE_ACCOUNT: 'vault_delete_account',
  VAULT_GET_SETTINGS: 'vault_get_settings',
  VAULT_UPDATE_SETTINGS: 'vault_update_settings',
  VAULT_PICK_KEY_FILE: 'vault_pick_key_file',
  VAULT_CHECK_PATH_EXISTS: 'vault_check_path_exists',
  VAULT_GENERATE_KEY: 'vault_generate_key',
  VAULT_LIST_KEYS: 'vault_list_keys',
  VAULT_DELETE_KEY: 'vault_delete_key',
  VAULT_MIGRATE_LEGACY: 'vault_migrate_legacy',

  // Updater
  CHECK_FOR_UPDATE: 'check_for_update',
  INSTALL_UPDATE: 'install_update',
  RESTART_APP: 'restart_app',

  // Plugins
  SCAN_PLUGINS: 'scan_plugins',
  ENABLE_PLUGIN: 'enable_plugin',
  DISABLE_PLUGIN: 'disable_plugin',
  DIALOG_RESPOND_FORM: 'dialog_respond_form',
  DIALOG_RESPOND_PROMPT: 'dialog_respond_prompt',
  DIALOG_RESPOND_CONFIRM: 'dialog_respond_confirm',
  GET_PLUGIN_MENU_ITEMS: 'get_plugin_menu_items',
  TRIGGER_PLUGIN_MENU_ACTION: 'trigger_plugin_menu_action',
  GET_PLUGIN_PANELS: 'get_plugin_panels',
  GET_PANEL_WIDGETS: 'get_panel_widgets',
  PLUGIN_WIDGET_EVENT: 'plugin_widget_event',
  REQUEST_PLUGIN_RENDER: 'request_plugin_render',
};

export const EVENTS = {
  // PTY
  PTY_OUTPUT: 'pty-output',
  PTY_EXIT: 'pty-exit',

  // Menu
  MENU_ACTION: 'menu-action',

  // Config
  CONFIG_CHANGED: 'config-changed',

  // SSH auth prompts
  SSH_HOST_KEY_PROMPT: 'ssh-host-key-prompt',
  SSH_PASSWORD_PROMPT: 'ssh-password-prompt',

  // Vault
  VAULT_LOCKED: 'vault-locked',
  VAULT_MIGRATION_NEEDED: 'vault-migration-needed',
  VAULT_AUTO_SAVE_PROMPT: 'vault-auto-save-prompt',

  // Transfers
  TRANSFER_PROGRESS: 'transfer-progress',

  // Updater
  UPDATE_AVAILABLE: 'update-available',
  UPDATE_PROGRESS: 'update-progress',

  // Plugins
  PLUGIN_PANEL_REGISTERED: 'plugin-panel-registered',
  PLUGIN_WIDGETS_UPDATED: 'plugin-widgets-updated',
  PLUGIN_PANELS_REMOVED: 'plugin-panels-removed',
  PLUGIN_NOTIFICATION: 'plugin-notification',
  PLUGIN_STATUS: 'plugin-status',
  PLUGIN_MENU_ITEM: 'plugin-menu-item',
  PLUGIN_FORM_DIALOG: 'plugin-form-dialog',
  PLUGIN_CONFIRM_DIALOG: 'plugin-confirm-dialog',
  PLUGIN_PROMPT_DIALOG: 'plugin-prompt-dialog',
  PLUGIN_WRITE_PTY: 'plugin-write-pty',
  PLUGIN_NEW_TAB: 'plugin-new-tab',
};
