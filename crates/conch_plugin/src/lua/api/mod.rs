//! Lua API table registration — `ui`, `app`, `session`, `net` tables with widget sugar.
//!
//! Each `ui.panel_*` function is syntactic sugar that constructs a `Widget`
//! enum variant and pushes it onto the widget accumulator. The accumulator
//! is drained after `render()` to produce the JSON widget tree.

mod app;
mod net;
mod session;
mod ui;

use std::cell::RefCell;
use std::sync::Arc;

use conch_plugin_sdk::widgets::*;
use mlua::prelude::*;

use crate::HostApi;

// ---------------------------------------------------------------------------
// Widget accumulator — thread-local stack of widget lists
// ---------------------------------------------------------------------------

/// Accumulates widgets during a `render()` call.
///
/// Uses a stack to support nested layout containers (`panel_horizontal`,
/// `panel_vertical`, etc.). The top of the stack is the current target.
pub struct WidgetAccumulator {
    stack: Vec<Vec<Widget>>,
}

impl WidgetAccumulator {
    pub fn new() -> Self {
        Self {
            stack: vec![vec![]],
        }
    }

    pub fn push_widget(&mut self, widget: Widget) {
        if let Some(top) = self.stack.last_mut() {
            top.push(widget);
        }
    }

    pub fn push_scope(&mut self) {
        self.stack.push(vec![]);
    }

    pub fn pop_scope(&mut self) -> Vec<Widget> {
        self.stack.pop().unwrap_or_default()
    }

    pub fn clear(&mut self) {
        self.stack.clear();
        self.stack.push(vec![]);
    }

    pub fn take_widgets(&mut self) -> Vec<Widget> {
        std::mem::take(self.stack.last_mut().unwrap_or(&mut vec![]))
    }
}

// ---------------------------------------------------------------------------
// Host API bridge — wraps the raw HostApi pointer for Lua access
// ---------------------------------------------------------------------------

/// Wraps an `Arc<dyn HostApi>` so it can be stored as Lua app data.
pub struct HostApiBridge {
    api: Arc<dyn HostApi>,
}

impl HostApiBridge {
    pub fn new(api: Arc<dyn HostApi>) -> Self {
        Self { api }
    }

    pub(super) fn api(&self) -> &dyn HostApi {
        &*self.api
    }
}

/// Stores the panel handle assigned during `register_panel()` so that
/// `ui.request_render()` can push widget updates without a RenderRequest.
pub struct PanelHandleStore {
    pub handle: Option<u64>,
}

impl PanelHandleStore {
    pub fn new() -> Self {
        Self { handle: None }
    }
}

// ---------------------------------------------------------------------------
// Registration — create and populate all Lua API tables
// ---------------------------------------------------------------------------

/// Register all API tables on the Lua VM.
///
/// Stores a `WidgetAccumulator` and `HostApiBridge` as app data.
pub fn register_all(lua: &Lua, host_api: Arc<dyn HostApi>) -> LuaResult<()> {
    lua.set_app_data(RefCell::new(WidgetAccumulator::new()));
    lua.set_app_data(HostApiBridge::new(host_api));
    lua.set_app_data(RefCell::new(PanelHandleStore::new()));

    ui::register_ui_table(lua)?;
    app::register_app_table(lua)?;
    session::register_session_table(lua)?;
    net::register_net_table(lua)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — widget accumulator + host API access
// ---------------------------------------------------------------------------

/// Borrow the widget accumulator, call the closure, then release.
pub(super) fn with_acc<F, R>(lua: &Lua, f: F) -> LuaResult<R>
where
    F: FnOnce(&mut WidgetAccumulator) -> R,
{
    with_acc_pub(lua, f)
}

/// Public version of `with_acc` for use by the runner module.
pub fn with_acc_pub<F, R>(lua: &Lua, f: F) -> LuaResult<R>
where
    F: FnOnce(&mut WidgetAccumulator) -> R,
{
    let cell = lua
        .app_data_ref::<RefCell<WidgetAccumulator>>()
        .ok_or_else(|| LuaError::RuntimeError("WidgetAccumulator not initialized".into()))?;
    let mut acc = cell.borrow_mut();
    Ok(f(&mut acc))
}

/// Borrow the HostApi trait object, call the closure.
pub(super) fn with_host_api<F, R>(lua: &Lua, f: F) -> LuaResult<R>
where
    F: FnOnce(&dyn HostApi) -> R,
{
    let bridge = lua
        .app_data_ref::<HostApiBridge>()
        .ok_or_else(|| LuaError::RuntimeError("HostApiBridge not initialized".into()))?;
    Ok(f(bridge.api()))
}

/// Take the accumulated widgets from the current render call.
pub fn take_widgets(lua: &Lua) -> LuaResult<Vec<Widget>> {
    with_acc(lua, |acc| acc.take_widgets())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_accumulator_basic() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_widget(Widget::Heading { text: "Hi".into() });
        let widgets = acc.take_widgets();
        assert_eq!(widgets.len(), 2);
    }

    #[test]
    fn widget_accumulator_nested_scope() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_scope();
        acc.push_widget(Widget::Heading {
            text: "Child".into(),
        });
        let children = acc.pop_scope();
        assert_eq!(children.len(), 1);
        // Parent still has the separator.
        let parent = acc.take_widgets();
        assert_eq!(parent.len(), 1);
    }

    #[test]
    fn widget_accumulator_clear() {
        let mut acc = WidgetAccumulator::new();
        acc.push_widget(Widget::Separator);
        acc.push_widget(Widget::Separator);
        acc.clear();
        let widgets = acc.take_widgets();
        assert!(widgets.is_empty());
    }
}
