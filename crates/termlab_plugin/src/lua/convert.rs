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
    fn lua_table_float_value_is_preserved_as_json_float() {
        let lua = lua();
        // `4.0` is a Lua float (not an integer) even though it is numerically
        // whole. It must serialize as a JSON float, not coerce into a JSON
        // integer, mirroring `number_kinds_are_preserved` for the reverse
        // (JSON->Lua) direction.
        let json = to_json(&lua, "return {count = 4.0}").unwrap();
        let count = json.get("count").expect("count key present");
        assert!(
            count.is_f64() && !count.is_i64(),
            "expected count to serialize as a JSON float, got: {count}"
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
        assert_eq!(
            describe(&lua, r#"null"#, "local v = ...; return type(v)"),
            "nil"
        );
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
