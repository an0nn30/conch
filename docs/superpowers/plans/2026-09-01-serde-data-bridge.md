# Serde Data Bridge Implementation Plan

> **Status: executed and merged in #108.** Kept as the record of how step 1 of
> the v2 core was built. The unchecked boxes below are the plan as written, not
> outstanding work.
>
> Two things diverged from the plan during execution, both corrected in the
> spec: the task briefs predicted their new tests would fail before
> implementation, but those tests call the bridge directly and pass as soon as
> it exists — the real red signal was the compile error after deleting each old
> converter. And a fourth converter, `set_lua_table_from_json_map` in
> `session.rs`, was not in the original spec.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the four hand-rolled Lua↔JSON converters in `termlab_plugin` with a single serde-backed bridge, removing the Lua-source-codegen path that corrupts event delivery.

**Architecture:** One new module, `crates/termlab_plugin/src/lua/convert.rs`, exposes four functions built on mlua's serde integration with explicitly pinned options. Every existing conversion call site is migrated to it, and the old converters are deleted. No plugin-facing API is renamed; the observable changes are that malformed payloads now produce errors instead of silent corruption, and nested values survive where they were previously dropped.

**Tech Stack:** Rust, mlua 0.10 (`lua54`, `vendored`, `send`, and newly `serialize`), serde_json.

**Spec:** `docs/superpowers/specs/2026-09-01-lua-plugin-api-v2-design.md` (section 2, "Data bridge & widget schema")

## Global Constraints

- mlua version is `0.10`; the `serialize` feature must be added to the existing feature list in the workspace root `Cargo.toml` line 51. Do not change the mlua version.
- Conversion options are fixed and must not be varied per call site:
  - Serialize (JSON→Lua): `set_array_metatable(false)`, `serialize_none_to_null(false)`, `serialize_unit_to_null(false)`
  - Deserialize (Lua→JSON): `DeserializeOptions::new()` defaults — `deny_unsupported_types` and `deny_recursive_tables` both remain enabled
- These conversion behaviors are verified against mlua 0.10 and are the expected results in tests:
  | Case | Result |
  |---|---|
  | Empty Lua table `{}` → JSON | `{}` (object) |
  | Lua `nil` value in a table | key absent from JSON |
  | JSON `null` → Lua | `nil` |
  | JSON integer → Lua | `math.type` reports `integer` |
  | JSON float → Lua | `math.type` reports `float` |
  | Array table → Lua | no metatable |
  | Lua function in payload | `Err`, message contains `unsupported value type` |
  | Recursive Lua table | `Err`, message contains `recursive table` |
  | Sparse table `{[1]='a',[3]='c'}` | `["a"]` — truncates at the gap |
- Per `CLAUDE.md`, any task changing the plugin-visible contract updates `docs/plugin-sdk.md` **in the same commit**. Never batch doc updates.
- Never add `Co-Authored-By` lines to commits. Work on a branch; never commit to `main`.
- Run the full crate suite with `cargo test -p termlab_plugin` before each commit.

## File Structure

**Create:**
- `crates/termlab_plugin/src/lua/convert.rs` — the only Lua↔JSON conversion code in the crate. Owns the serde options, the four public functions, and the convention tests.

**Modify:**
- `Cargo.toml` (root, line 51) — add the `serialize` mlua feature
- `crates/termlab_plugin/src/lua/mod.rs` — declare `pub mod convert;`
- `crates/termlab_plugin/src/lua/runner.rs` — replace codegen dispatch; delete `json_to_lua_literal` and `json_value_to_lua_literal`
- `crates/termlab_plugin/src/lua/api/ui.rs` — migrate 5 call sites; delete `lua_value_to_json`, `json_to_lua_table`, `json_to_lua_value`
- `crates/termlab_plugin/src/lua/api/app.rs` — migrate 3 call sites
- `crates/termlab_plugin/src/lua/api/session.rs` — migrate 2 call sites; delete `set_lua_table_from_json_map`
- `docs/plugin-sdk.md` — document conversion behavior and the changed contracts

---

### Task 1: The convert module

**Files:**
- Create: `crates/termlab_plugin/src/lua/convert.rs`
- Modify: `Cargo.toml:51`, `crates/termlab_plugin/src/lua/mod.rs`

**Interfaces:**
- Consumes: nothing
- Produces:
  - `pub fn json_to_lua(lua: &Lua, value: &serde_json::Value) -> LuaResult<LuaValue>`
  - `pub fn json_str_to_lua(lua: &Lua, json: &str) -> LuaResult<LuaValue>`
  - `pub fn lua_to_json(lua: &Lua, value: LuaValue) -> LuaResult<serde_json::Value>`
  - `pub fn lua_to_json_string(lua: &Lua, value: LuaValue) -> LuaResult<String>`
  - `pub fn merge_json_into_table(lua: &Lua, tbl: &LuaTable, value: &serde_json::Value) -> LuaResult<()>`

- [ ] **Step 1: Enable the mlua serialize feature**

In the workspace root `Cargo.toml`, line 51, change:

```toml
mlua = { version = "0.10", features = ["lua54", "vendored", "send"] }
```

to:

```toml
mlua = { version = "0.10", features = ["lua54", "vendored", "send", "serialize"] }
```

- [ ] **Step 2: Write the failing convention tests**

Create `crates/termlab_plugin/src/lua/convert.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        Lua::new()
    }

    /// Evaluate a Lua expression and convert the result to JSON.
    fn to_json(lua: &Lua, src: &str) -> LuaResult<serde_json::Value> {
        let value: LuaValue = lua.load(src).eval()?;
        lua_to_json(lua, value)
    }

    /// Convert JSON text to Lua, then describe it with a Lua expression.
    fn describe(lua: &Lua, json: &str, expr: &str) -> String {
        let value = json_str_to_lua(lua, json).expect("json_str_to_lua");
        lua.load(expr).call(value).expect("describe")
    }

    #[test]
    fn empty_table_becomes_empty_object() {
        let lua = lua();
        assert_eq!(to_json(&lua, "return {}").unwrap(), serde_json::json!({}));
    }

    #[test]
    fn sequence_becomes_array() {
        let lua = lua();
        assert_eq!(
            to_json(&lua, "return {1,2,3}").unwrap(),
            serde_json::json!([1, 2, 3])
        );
    }

    #[test]
    fn nil_valued_key_is_absent() {
        let lua = lua();
        assert_eq!(
            to_json(&lua, "return {a=1, b=nil}").unwrap(),
            serde_json::json!({"a": 1})
        );
    }

    #[test]
    fn nested_structures_round_trip() {
        let lua = lua();
        assert_eq!(
            to_json(&lua, "return {nested={a={1,2}}}").unwrap(),
            serde_json::json!({"nested": {"a": [1, 2]}})
        );
    }

    #[test]
    fn function_in_payload_is_an_error() {
        let lua = lua();
        let err = to_json(&lua, "return {f=function() end}").unwrap_err();
        assert!(
            err.to_string().contains("unsupported value type"),
            "expected unsupported-type error, got: {err}"
        );
    }

    #[test]
    fn recursive_table_is_an_error() {
        let lua = lua();
        let err = to_json(&lua, "local t={}; t.self=t; return t").unwrap_err();
        assert!(
            err.to_string().contains("recursive table"),
            "expected recursive-table error, got: {err}"
        );
    }

    #[test]
    fn sparse_table_truncates_at_the_gap() {
        // Documented edge: mlua stops at the first hole rather than erroring.
        let lua = lua();
        assert_eq!(
            to_json(&lua, "return {[1]='a', [3]='c'}").unwrap(),
            serde_json::json!(["a"])
        );
    }

    #[test]
    fn json_null_becomes_nil_not_a_sentinel() {
        let lua = lua();
        assert_eq!(describe(&lua, r#"null"#, "local v = ...; return type(v)"), "nil");
        assert_eq!(
            describe(&lua, r#"{"a":null}"#, "local v = ...; return type(v.a)"),
            "nil"
        );
    }

    #[test]
    fn number_kinds_are_preserved() {
        let lua = lua();
        assert_eq!(
            describe(
                &lua,
                r#"{"i":3,"f":1.5}"#,
                "local v = ...; return math.type(v.i)..'/'..math.type(v.f)"
            ),
            "integer/float"
        );
    }

    #[test]
    fn array_tables_have_no_metatable() {
        let lua = lua();
        assert_eq!(
            describe(
                &lua,
                r#"[1,2,3]"#,
                "local v = ...; return tostring(getmetatable(v) ~= nil)"
            ),
            "false"
        );
    }

    #[test]
    fn json_str_to_lua_rejects_malformed_input() {
        let lua = lua();
        let err = json_str_to_lua(&lua, "{not json").unwrap_err();
        assert!(
            err.to_string().contains("invalid JSON payload"),
            "expected parse error, got: {err}"
        );
    }

    #[test]
    fn lua_to_json_string_emits_text() {
        let lua = lua();
        let value: LuaValue = lua.load("return {a=1}").eval().unwrap();
        assert_eq!(lua_to_json_string(&lua, value).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn merge_sets_keys_onto_an_existing_table() {
        let lua = lua();
        let tbl = lua.create_table().unwrap();
        tbl.set("keep", "original").unwrap();
        merge_json_into_table(
            &lua,
            &tbl,
            &serde_json::json!({"host": "example", "port": 22, "opts": {"a": 1}}),
        )
        .unwrap();

        assert_eq!(tbl.get::<String>("keep").unwrap(), "original");
        assert_eq!(tbl.get::<String>("host").unwrap(), "example");
        assert_eq!(tbl.get::<i64>("port").unwrap(), 22);
        let opts: LuaTable = tbl.get("opts").unwrap();
        assert_eq!(opts.get::<i64>("a").unwrap(), 1);
    }

    #[test]
    fn merge_ignores_non_object_json() {
        let lua = lua();
        let tbl = lua.create_table().unwrap();
        merge_json_into_table(&lua, &tbl, &serde_json::json!([1, 2])).unwrap();
        assert_eq!(tbl.len().unwrap(), 0);
    }
}
```

Add the module declaration to `crates/termlab_plugin/src/lua/mod.rs`, after the doc comment block:

```rust
pub mod api;
pub mod convert;
pub mod metadata;
pub mod runner;
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p termlab_plugin --lib convert`
Expected: compile error — `cannot find function json_to_lua in this scope` (and the other four).

- [ ] **Step 4: Write the implementation**

Prepend this above the test module in `crates/termlab_plugin/src/lua/convert.rs`:

```rust
//! The single Lua↔JSON conversion bridge for plugin data payloads.
//!
//! Widget trees do NOT pass through here — they carry Lua functions, which
//! serde cannot represent, and are handled by the typed widget walker. This
//! module is for plain data: bus payloads, config values, service arguments
//! and results, and dialog payloads.
//!
//! Options are pinned in one place on purpose. Varying them per call site is
//! how a codebase ends up with payloads that round-trip differently depending
//! on which API carried them.

use mlua::prelude::*;
use mlua::{DeserializeOptions, LuaSerdeExt, SerializeOptions};
use serde_json::Value as JsonValue;

/// JSON→Lua options.
///
/// `serialize_*_to_null(false)` is required: mlua's default maps JSON null to
/// a lightuserdata sentinel, so `value == nil` would be false in plugin code.
/// `set_array_metatable(false)` keeps arrays as plain Lua tables.
fn serialize_options() -> SerializeOptions {
    SerializeOptions::new()
        .set_array_metatable(false)
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
}

/// Lua→JSON options. Defaults reject functions and cycles, both of which are
/// plugin bugs that previously produced silent corruption or a stack overflow.
fn deserialize_options() -> DeserializeOptions {
    DeserializeOptions::new()
}

/// Convert a JSON value into a Lua value.
pub fn json_to_lua(lua: &Lua, value: &JsonValue) -> LuaResult<LuaValue> {
    lua.to_value_with(value, serialize_options())
}

/// Parse JSON text and convert it into a Lua value.
pub fn json_str_to_lua(lua: &Lua, json: &str) -> LuaResult<LuaValue> {
    let value: JsonValue = serde_json::from_str(json)
        .map_err(|e| LuaError::RuntimeError(format!("invalid JSON payload: {e}")))?;
    json_to_lua(lua, &value)
}

/// Convert a Lua value into a JSON value.
pub fn lua_to_json(lua: &Lua, value: LuaValue) -> LuaResult<JsonValue> {
    lua.from_value_with(value, deserialize_options())
}

/// Convert a Lua value into a JSON string.
pub fn lua_to_json_string(lua: &Lua, value: LuaValue) -> LuaResult<String> {
    let json = lua_to_json(lua, value)?;
    serde_json::to_string(&json)
        .map_err(|e| LuaError::RuntimeError(format!("encode JSON payload: {e}")))
}

/// Set each key of a JSON object onto an existing Lua table.
///
/// Used where the host merges a response into a table that already carries
/// caller-supplied defaults. Non-object JSON is ignored.
pub fn merge_json_into_table(lua: &Lua, tbl: &LuaTable, value: &JsonValue) -> LuaResult<()> {
    let JsonValue::Object(map) = value else {
        return Ok(());
    };
    for (key, item) in map {
        tbl.set(key.as_str(), json_to_lua(lua, item)?)?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_plugin --lib convert`
Expected: PASS, 14 tests.

- [ ] **Step 6: Run the whole crate suite**

Run: `cargo test -p termlab_plugin`
Expected: PASS — nothing else is wired to `convert` yet, so no existing test changes.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/termlab_plugin/src/lua/convert.rs crates/termlab_plugin/src/lua/mod.rs
git commit -m "Add serde-backed Lua/JSON conversion bridge

Single module with pinned serialize and deserialize options, replacing
what will be four hand-rolled converters. Options are fixed here rather
than per call site so payloads round-trip identically regardless of which
API carried them."
```

---

### Task 2: Replace the event codegen path

This is the security-relevant task. `dispatch_event` currently builds Lua *source* from JSON and `load()`s it. Object keys escape `"` but not `\`, so a key ending in a backslash produces a malformed literal: the eval fails, and the plugin silently receives a raw JSON string instead of a table.

**Files:**
- Modify: `crates/termlab_plugin/src/lua/runner.rs` — `dispatch_event` (~line 232), `dispatch_event_json_raw` (~line 260); delete `json_to_lua_literal` and `json_value_to_lua_literal` and their five tests
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::lua::convert::{json_str_to_lua, json_to_lua}` from Task 1
- Produces: no new public interface

- [ ] **Step 1: Write the failing regression test**

Add to the `tests` module in `crates/termlab_plugin/src/lua/runner.rs`:

```rust
#[test]
fn bus_event_key_with_backslash_is_delivered_intact() {
    let lua = sandboxed_lua();
    lua.load(
        r#"
        received_type = nil
        received_key = nil
        function on_event(e)
            received_type = type(e.data)
            if type(e.data) == 'table' then
                for k, _ in pairs(e.data) do received_key = k end
            end
        end
    "#,
    )
    .exec()
    .unwrap();

    // A key ending in a backslash breaks the generated Lua literal: the
    // escape consumes the closing quote of the bracketed key.
    let event = PluginEvent::BusEvent {
        event_type: "test".into(),
        data: serde_json::json!({ "danger\\": 1 }),
    };
    dispatch_event(&lua, &event);

    assert_eq!(
        lua.globals().get::<String>("received_type").unwrap(),
        "table",
        "event payload must arrive as a table, not fall back to a raw string"
    );
    assert_eq!(
        lua.globals().get::<String>("received_key").unwrap(),
        "danger\\",
        "key must arrive byte-identical"
    );
}

#[test]
fn bus_event_key_with_quotes_and_newlines_is_delivered_intact() {
    let lua = sandboxed_lua();
    lua.load(
        r#"
        keys = {}
        function on_event(e)
            for k, _ in pairs(e.data) do keys[#keys+1] = k end
            table.sort(keys)
        end
    "#,
    )
    .exec()
    .unwrap();

    let event = PluginEvent::BusEvent {
        event_type: "test".into(),
        data: serde_json::json!({ "a\"b": 1, "c\nd": 2 }),
    };
    dispatch_event(&lua, &event);

    let keys: Vec<String> = lua.globals().get::<LuaTable>("keys").unwrap()
        .sequence_values().collect::<LuaResult<_>>().unwrap();
    assert_eq!(keys, vec!["a\"b".to_string(), "c\nd".to_string()]);
}

#[test]
fn bus_event_null_value_arrives_as_nil() {
    let lua = sandboxed_lua();
    lua.load(
        r#"
        was_nil = nil
        function on_event(e) was_nil = (e.data.maybe == nil) end
    "#,
    )
    .exec()
    .unwrap();

    let event = PluginEvent::BusEvent {
        event_type: "test".into(),
        data: serde_json::json!({ "maybe": null }),
    };
    dispatch_event(&lua, &event);

    assert!(lua.globals().get::<bool>("was_nil").unwrap());
}
```

Note: `sandboxed_lua()` already exists in this test module. These tests need `WidgetAccumulator` app data only if they render, which they do not.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_plugin --lib runner::tests::bus_event`
Expected: FAIL. `bus_event_key_with_backslash_is_delivered_intact` fails with `received_type == "string"` — the generated literal did not parse, so the fallback passed raw JSON text.

- [ ] **Step 3: Replace the conversion in both dispatch paths**

In `dispatch_event`, replace the literal-building block:

```rust
    let json = match serde_json::to_string(event) {
        Ok(j) => j,
        Err(_) => return,
    };

    // Parse the JSON into a Lua table so the plugin gets a native table.
    let lua_literal = json_to_lua_literal(&json);
    let Ok(tbl) = lua
        .load(&format!("return {}", lua_literal))
        .eval::<LuaTable>()
    else {
        log::warn!("dispatch_event: failed to eval lua literal: {lua_literal}");
        // Fallback: pass as string.
        with_instruction_limit(lua, || {
            if let Err(e) = on_event.call::<()>(json) {
                log::warn!("dispatch_event: on_event(string) error: {e}");
            }
        });
        return;
    };

    with_instruction_limit(lua, || {
        if let Err(e) = on_event.call::<()>(tbl) {
            log::warn!("dispatch_event: on_event(table) error: {e}");
        }
    });
```

with:

```rust
    let value = match serde_json::to_value(event) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event: failed to encode event: {e}");
            return;
        }
    };

    let payload = match convert::json_to_lua(lua, &value) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event: failed to convert event payload: {e}");
            return;
        }
    };

    with_instruction_limit(lua, || {
        if let Err(e) = on_event.call::<()>(payload) {
            log::warn!("dispatch_event: on_event error: {e}");
        }
    });
```

In `dispatch_event_json_raw`, replace the equivalent block with:

```rust
    let payload = match convert::json_str_to_lua(lua, json) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("dispatch_event_json_raw: failed to convert event payload: {e}");
            return;
        }
    };

    with_instruction_limit(lua, || {
        if let Err(e) = on_event.call::<()>(payload) {
            log::warn!("dispatch_event_json_raw: on_event error: {e}");
        }
    });
```

Add the import near the top of the file, beside the existing `use crate::lua::api;`:

```rust
use crate::lua::convert;
```

The string fallback is deliberately gone. It papered over conversion failures by handing plugins a type they did not expect; a warning log is the honest outcome.

- [ ] **Step 4: Delete the codegen functions and their tests**

Delete from `crates/termlab_plugin/src/lua/runner.rs`:
- `fn json_to_lua_literal`
- `fn json_value_to_lua_literal`
- These tests, which only exercised the deleted functions: `json_to_lua_literal_object`, `json_to_lua_literal_array`, `json_to_lua_literal_nested`, `json_to_lua_literal_string_escaping`, `json_to_lua_literal_null`

Keep `json_to_lua_literal_menu_action` but rewrite it against the new path, since it asserts a real behavior (menu actions reach `on_event` with the right shape):

```rust
#[test]
fn menu_action_event_reaches_on_event() {
    let lua = sandboxed_lua();
    lua.load(
        r#"
        seen_kind = nil
        seen_action = nil
        function on_event(e)
            seen_kind = e.kind
            seen_action = e.action
        end
    "#,
    )
    .exec()
    .unwrap();

    let event = PluginEvent::MenuAction { action: "trigger_notification".into() };
    dispatch_event(&lua, &event);

    assert_eq!(lua.globals().get::<String>("seen_kind").unwrap(), "menu_action");
    assert_eq!(
        lua.globals().get::<String>("seen_action").unwrap(),
        "trigger_notification"
    );
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_plugin --lib runner`
Expected: PASS. The three new `bus_event_*` tests and the rewritten menu-action test all pass; no test references a deleted function.

- [ ] **Step 6: Update the SDK documentation**

In `docs/plugin-sdk.md`, find the "Plugin Events" section (around line 1360). Add after the event-kind list:

```markdown
Event payloads are converted from JSON to native Lua values by the host. A JSON
`null` arrives as Lua `nil` (so `if e.data.field == nil then` works as written),
integers stay integers, and arrays arrive as plain tables with no metatable.

If a payload cannot be converted, the event is dropped and the reason is logged.
Plugins never receive a partially converted payload, and never receive a raw
JSON string in place of a table.
```

- [ ] **Step 7: Run the whole crate suite and commit**

Run: `cargo test -p termlab_plugin`
Expected: PASS.

```bash
git add crates/termlab_plugin/src/lua/runner.rs docs/plugin-sdk.md
git commit -m "Convert plugin events through serde instead of generated Lua

Events reached Lua by building Lua source and load()ing it. Object keys
escaped quotes but not backslashes, so a key ending in a backslash produced
a malformed literal: the eval failed and the plugin silently received a raw
JSON string instead of a table.

Drop the string fallback with the codegen. A conversion failure now logs and
drops the event rather than handing plugins an unexpected type."
```

---

### Task 3: Migrate the Lua→JSON call sites

**Files:**
- Modify: `crates/termlab_plugin/src/lua/api/app.rs` — lines ~58 (`publish`), ~171 (`register_settings_section`), ~189 (`query_plugin`)
- Modify: `crates/termlab_plugin/src/lua/api/ui.rs` — line ~438 (`open_docked_view`), line ~734 (`build_form_json`); delete `lua_value_to_json` and its tests
- Modify: `docs/plugin-sdk.md`

**Interfaces:**
- Consumes: `crate::lua::convert::{lua_to_json, lua_to_json_string}` from Task 1
- Produces: no new public interface

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/termlab_plugin/src/lua/api/ui.rs`:

```rust
#[test]
fn publish_payload_rejects_functions_with_a_clear_error() {
    // A function in a published payload is a plugin bug. It used to become
    // JSON null silently; it must now surface.
    let lua = Lua::new();
    let value: LuaValue = lua.load("return {cb=function() end}").eval().unwrap();
    let err = crate::lua::convert::lua_to_json_string(&lua, value).unwrap_err();
    assert!(err.to_string().contains("unsupported value type"));
}

#[test]
fn publish_payload_preserves_nested_tables() {
    let lua = Lua::new();
    let value: LuaValue = lua
        .load("return {list={1,2}, map={k='v'}}")
        .eval()
        .unwrap();
    let json = crate::lua::convert::lua_to_json_string(&lua, value).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, serde_json::json!({"list": [1, 2], "map": {"k": "v"}}));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p termlab_plugin --lib ui::tests::publish_payload`
Expected: FAIL — compile error, `convert` is not imported in `ui.rs` yet, or the test module cannot resolve the path.

- [ ] **Step 3: Migrate the call sites**

In `crates/termlab_plugin/src/lua/api/app.rs`, replace the import at line 5:

```rust
use super::ui::lua_value_to_json;
```

with:

```rust
use crate::lua::convert::{lua_to_json, lua_to_json_string};
```

Then, in `publish` (~line 58), replace:

```rust
            let data_json = serde_json::to_string(&lua_value_to_json(data)?)
                .unwrap_or_else(|_| "{}".to_string());
```

with:

```rust
            let data_json = lua_to_json_string(lua, data)?;
```

In `register_settings_section` (~line 171), replace:

```rust
            let section_json_value = lua_value_to_json(section)?;
            if !section_json_value.is_object() {
                return Err(LuaError::RuntimeError(
                    "register_settings_section expects a table/object".into(),
                ));
            }
            let section_json = serde_json::to_string(&section_json_value)
                .map_err(|e| LuaError::RuntimeError(format!("encode settings section: {e}")))?;
```

with:

```rust
            let section_json_value = lua_to_json(lua, section)?;
            if !section_json_value.is_object() {
                return Err(LuaError::RuntimeError(
                    "register_settings_section expects a table/object".into(),
                ));
            }
            let section_json = serde_json::to_string(&section_json_value)
                .map_err(|e| LuaError::RuntimeError(format!("encode settings section: {e}")))?;
```

In `query_plugin` (~line 189), replace:

```rust
                let args_json = match args {
                    Some(v) => serde_json::to_string(&lua_value_to_json(v)?)
                        .unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
```

with:

```rust
                let args_json = match args {
                    Some(v) => lua_to_json_string(lua, v)?,
                    None => "null".to_string(),
                };
```

In `crates/termlab_plugin/src/lua/api/ui.rs`, add the import beside the existing `use` statements at the top:

```rust
use crate::lua::convert::{lua_to_json, lua_to_json_string};
```

In `open_docked_view` (~line 438), replace:

```rust
            let req_json = serde_json::to_string(&lua_value_to_json(opts)?)
                .unwrap_or_else(|_| "{}".to_string());
```

with:

```rust
            let req_json = lua_to_json_string(lua, opts)?;
```

`build_form_json` (~line 722) is a free function with no `&Lua` parameter, so it needs one. Change its signature and the single conversion inside it:

```rust
fn build_form_json(lua: &Lua, title: &str, fields: &LuaTable) -> LuaResult<String> {
```

and inside, replace:

```rust
                    obj.insert(key.to_string(), lua_value_to_json(v)?);
```

with:

```rust
                    obj.insert(key.to_string(), lua_to_json(lua, v)?);
```

Update its call sites — find them with `grep -n 'build_form_json' crates/termlab_plugin/src/lua/api/ui.rs` and pass `lua` as the first argument.

- [ ] **Step 4: Delete `lua_value_to_json`**

Delete `pub(super) fn lua_value_to_json` (~line 822) from `ui.rs`, and delete the test `lua_value_to_json_primitives` (~line 1091), whose behavior is now covered by the convention tests in `convert.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_plugin`
Expected: PASS. If a call site was missed, the build fails with `cannot find function lua_value_to_json` — fix and re-run.

- [ ] **Step 6: Update the SDK documentation**

In `docs/plugin-sdk.md`, in the `app` table section (around line 1067), update the `app.publish` entry to note the new error behavior:

```markdown
`app.publish(event_type, data)` — publish an event on the bus. `data` is
converted to JSON: nested tables are preserved, `nil`-valued keys are omitted,
and integers stay integers. Passing a function or a table containing a cycle
raises a Lua error rather than silently publishing a corrupted payload.
```

Apply the same note to `app.query_plugin(target, method, args)`.

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_plugin/src/lua/api/app.rs crates/termlab_plugin/src/lua/api/ui.rs docs/plugin-sdk.md
git commit -m "Convert Lua payloads to JSON through the serde bridge

Replaces lua_value_to_json across publish, settings sections, plugin
queries, docked views, and form dialogs. Functions and cyclic tables now
raise instead of silently serializing as null or recursing until the stack
overflows."
```

---

### Task 4: Migrate the JSON→Lua call sites

**Files:**
- Modify: `crates/termlab_plugin/src/lua/api/ui.rs` — line ~446 (`open_docked_view` result), line ~764 (`call_show_form` result); delete `json_to_lua_table` and `json_to_lua_value` and their tests

**Interfaces:**
- Consumes: `crate::lua::convert::json_to_lua` from Task 1
- Produces: no new public interface

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/termlab_plugin/src/lua/api/ui.rs`:

```rust
#[test]
fn host_json_responses_convert_to_native_lua() {
    let lua = Lua::new();
    let json = serde_json::json!({
        "id": "view-1",
        "count": 3,
        "ratio": 0.5,
        "tags": ["a", "b"],
        "missing": null
    });
    let value = crate::lua::convert::json_to_lua(&lua, &json).unwrap();
    let summary: String = lua
        .load(
            r#"
            local v = ...
            return v.id .. '/' .. math.type(v.count) .. '/' .. math.type(v.ratio)
                .. '/' .. #v.tags .. '/' .. type(v.missing)
        "#,
        )
        .call(value)
        .unwrap();
    assert_eq!(summary, "view-1/integer/float/2/nil");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p termlab_plugin --lib ui::tests::host_json_responses`
Expected: FAIL — compile error until `convert` is reachable from this module (it is imported in Task 3; if executing tasks out of order, add `use crate::lua::convert;`).

- [ ] **Step 3: Migrate the call sites**

In `open_docked_view` (~line 444), replace:

```rust
            let value: serde_json::Value =
                serde_json::from_str(&result_json).unwrap_or(serde_json::Value::Null);
            let tbl = json_to_lua_table(lua, &value)?;
            Ok(Some(tbl))
```

with the following. The surrounding closure returns `LuaResult<Option<LuaTable>>`, so destructure rather than widening the return type:

```rust
            let value: serde_json::Value =
                serde_json::from_str(&result_json).unwrap_or(serde_json::Value::Null);
            let converted = convert::json_to_lua(lua, &value)?;
            let LuaValue::Table(tbl) = converted else {
                return Ok(None);
            };
            Ok(Some(tbl))
```

In `call_show_form` (~line 762), replace:

```rust
    let json_value: serde_json::Value =
        serde_json::from_str(&result_str).unwrap_or(serde_json::Value::Null);
    let tbl = json_to_lua_table(lua, &json_value)?;
    Ok(Some(tbl))
```

with:

```rust
    let json_value: serde_json::Value =
        serde_json::from_str(&result_str).unwrap_or(serde_json::Value::Null);
    let converted = convert::json_to_lua(lua, &json_value)?;
    let LuaValue::Table(tbl) = converted else {
        return Ok(None);
    };
    Ok(Some(tbl))
```

Both host responses are objects in practice; a non-object response now yields `nil` to the plugin rather than an empty table, which is a truer signal that nothing came back.

- [ ] **Step 4: Delete the old converters**

Delete `fn json_to_lua_table` (~line 785) and `fn json_to_lua_value` (~line 795) from `ui.rs`, plus the test that exercised them directly (~line 1114, the one calling `json_to_lua_table`).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_plugin`
Expected: PASS.

- [ ] **Step 6: Commit**

No SDK documentation change is needed for this task: the plugin-visible contract for form and docked-view results is unchanged except that a non-object response is now `nil` instead of an empty table, which is already covered by the conversion note added in Task 2. Verify that is true by re-reading the `ui.form` and `ui.open_docked_view` entries in `docs/plugin-sdk.md`; if either promises a table unconditionally, correct it in this commit.

```bash
git add crates/termlab_plugin/src/lua/api/ui.rs
git commit -m "Convert host JSON responses through the serde bridge

Deletes the last two hand-rolled JSON to Lua converters. Non-object
responses now reach plugins as nil rather than an empty table."
```

---

### Task 5: Migrate the session response merge

`session.exec_active` and `session.current` merge a host JSON response into a table that already holds defaults. The existing helper handles only strings, booleans, and numbers — nested objects and arrays are silently dropped.

**Files:**
- Modify: `crates/termlab_plugin/src/lua/api/session.rs` — lines ~62, ~102; delete `set_lua_table_from_json_map`
- Modify: `docs/plugin-sdk.md`

**Interfaces:**
- Consumes: `crate::lua::convert::merge_json_into_table` from Task 1
- Produces: no new public interface

- [ ] **Step 1: Write the failing test**

Add a `tests` module at the bottom of `crates/termlab_plugin/src/lua/api/session.rs` (the file has none today):

```rust
#[cfg(test)]
mod tests {
    use crate::lua::convert::merge_json_into_table;
    use mlua::prelude::*;

    #[test]
    fn merge_preserves_caller_defaults_and_adds_nested_values() {
        let lua = Lua::new();
        let tbl = lua.create_table().unwrap();
        tbl.set("platform", "macos").unwrap();

        merge_json_into_table(
            &lua,
            &tbl,
            &serde_json::json!({
                "type": "ssh",
                "host": "example.com",
                "port": 22,
                "forwards": [{"local_port": 8080}]
            }),
        )
        .unwrap();

        assert_eq!(tbl.get::<String>("platform").unwrap(), "macos");
        assert_eq!(tbl.get::<String>("type").unwrap(), "ssh");
        assert_eq!(tbl.get::<i64>("port").unwrap(), 22);

        // Nested values used to be dropped entirely.
        let forwards: LuaTable = tbl.get("forwards").unwrap();
        let first: LuaTable = forwards.get(1).unwrap();
        assert_eq!(first.get::<i64>("local_port").unwrap(), 8080);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p termlab_plugin --lib session`
Expected: FAIL — compile error, `merge_json_into_table` is not yet used here, or the test module does not exist.

- [ ] **Step 3: Migrate both call sites**

In `exec_active` (~line 60), replace:

```rust
                    if let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(&json) {
                        set_lua_table_from_json_map(&tbl, map)?;
```

with:

```rust
                    if let Ok(value @ JsonValue::Object(_)) =
                        serde_json::from_str::<JsonValue>(&json)
                    {
                        merge_json_into_table(lua, &tbl, &value)?;
```

In `current` (~line 100), replace:

```rust
            if let Some(json) = with_host_api(lua, |api| api.get_active_session())?
                && let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(&json)
            {
                set_lua_table_from_json_map(&tbl, map)?;
            }
```

with:

```rust
            if let Some(json) = with_host_api(lua, |api| api.get_active_session())?
                && let Ok(value @ JsonValue::Object(_)) = serde_json::from_str::<JsonValue>(&json)
            {
                merge_json_into_table(lua, &tbl, &value)?;
            }
```

Add the import near the top of the file:

```rust
use crate::lua::convert::merge_json_into_table;
```

- [ ] **Step 4: Delete the old helper**

Delete `fn set_lua_table_from_json_map` from the bottom of `session.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p termlab_plugin`
Expected: PASS.

- [ ] **Step 6: Update the SDK documentation**

In `docs/plugin-sdk.md`, in the `session` table section (around line 1168), update the `session.current()` and `session.exec_active()` entries:

```markdown
Fields from the host response are merged into the returned table. Nested
objects and arrays are preserved — earlier versions dropped anything that was
not a string, boolean, or number.
```

- [ ] **Step 7: Commit**

```bash
git add crates/termlab_plugin/src/lua/api/session.rs docs/plugin-sdk.md
git commit -m "Merge session responses through the serde bridge

The previous helper copied only scalars, so nested objects and arrays in
host responses were silently dropped before reaching plugins."
```

---

### Task 6: Verify the bridge is the only path, and review the docs

This task adds no behavior. It is the branch-level verification gate, including the documentation review `CLAUDE.md` requires.

**Files:**
- Modify: `docs/plugin-sdk.md` (only if the review finds drift)

- [ ] **Step 1: Prove no hand-rolled converter survives**

Run:

```bash
grep -rn 'lua_value_to_json\|json_to_lua_table\|json_to_lua_value\|json_to_lua_literal\|json_value_to_lua_literal\|set_lua_table_from_json_map' crates/ --include=*.rs
```

Expected: no output. Any hit is a missed call site or a stale test.

- [ ] **Step 2: Prove conversion options are defined in exactly one place**

Run:

```bash
grep -rn 'SerializeOptions\|DeserializeOptions' crates/ --include=*.rs
```

Expected: hits only in `crates/termlab_plugin/src/lua/convert.rs`. Options defined elsewhere mean a call site can diverge.

- [ ] **Step 3: Review the SDK documentation against the actual surface**

Read `docs/plugin-sdk.md` sections "Lua API Reference", "Lua Signatures", and "Plugin Events". Check both directions:

- Every documented `app.*`, `ui.*`, `session.*`, and `net.*` function still exists. Cross-check against:
  ```bash
  grep -oE '^\s+"[a-z_0-9]+",$' crates/termlab_plugin/src/lua/api/*.rs | tr -d ' ",' | sort -u
  ```
- Every function in that list has a documentation entry.
- The conversion notes added in Tasks 2, 3, and 5 describe what the code now does.

Fix any drift found. Record what was checked in the commit message even if nothing changed.

- [ ] **Step 4: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add docs/plugin-sdk.md
git commit -m "Review plugin SDK docs against the converted API surface

Verified no hand-rolled converters remain, conversion options are defined
only in convert.rs, and every documented Lua function exists and every
existing function is documented."
```

If Step 3 found no drift, skip the commit and note the verification in the branch summary instead.

---

## Self-Review

**Spec coverage.** Section 2's data-path requirements map to tasks as follows: the single bridge (Task 1), deletion of `json_value_to_lua_literal` (Task 2), deletion of `json_to_lua_value` / `json_to_lua_table` / `lua_value_to_json` (Tasks 3–4), and the pinned serde conventions (Task 1 tests, enforced by Task 6 Step 2). `set_lua_table_from_json_map` is a fourth converter the spec did not name; Task 5 covers it.

Section 2's remaining content — the typed widget walker, positional error messages, and schema deletions — is deliberately **out of scope** for this plan. Those belong to piece 2 (widget schema), which depends on this bridge existing.

**Placeholder scan.** No TBDs. Every code step contains the actual code. Task 6 Step 3 is a review with concrete commands and explicit pass criteria rather than "check the docs."

**Type consistency.** `json_to_lua`, `json_str_to_lua`, `lua_to_json`, `lua_to_json_string`, and `merge_json_into_table` are defined in Task 1 and used with those exact names and signatures in Tasks 2–5. One drafting error was caught and fixed inline: Task 4 Step 3 initially referenced a nonexistent `json_to_lua_value_from_str`; it now destructures the `LuaValue` returned by `json_to_lua`.

**Known behavior changes** (all intentional, all documented in the tasks that cause them):
1. Functions in payloads error instead of becoming `null`.
2. Cyclic tables error instead of overflowing the stack.
3. Event conversion failure drops the event instead of passing a raw JSON string.
4. Non-object dialog and docked-view responses yield `nil` instead of `{}`.
5. Nested values in session responses are preserved instead of dropped.
