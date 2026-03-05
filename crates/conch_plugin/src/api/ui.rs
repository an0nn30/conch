use mlua::{Lua, Result as LuaResult, Value};

use super::{FormField, PluginCommand, PluginContext, PluginResponse};

/// Register the `ui` table into the Lua state.
pub fn register(lua: &Lua, ctx: PluginContext) -> LuaResult<()> {
    let ui = lua.create_table()?;

    // ui.append(text) — append text to plugin output panel
    let ctx_append = ctx.clone();
    ui.set(
        "append",
        lua.create_function(move |_lua, text: String| {
            ctx_append.send_fire_and_forget(PluginCommand::UiAppend(text));
            Ok(())
        })?,
    )?;

    // ui.clear() — clear the plugin output panel
    let ctx_clear = ctx.clone();
    ui.set(
        "clear",
        lua.create_function(move |_lua, ()| {
            ctx_clear.send_fire_and_forget(PluginCommand::UiClear);
            Ok(())
        })?,
    )?;

    // ui.form(title, fields_table) — show a form dialog, returns table or nil
    let ctx_form = ctx.clone();
    ui.set(
        "form",
        lua.create_async_function(move |lua, (title, fields_table): (String, mlua::Table)| {
            let ctx = ctx_form.clone();
            async move {
                let fields = parse_form_fields(&fields_table)?;
                let resp = ctx
                    .send_command(PluginCommand::ShowForm { title, fields })
                    .await;
                match resp {
                    PluginResponse::FormResult(Some(map)) => {
                        let t = lua.create_table()?;
                        for (k, v) in map {
                            t.set(k, v)?;
                        }
                        Ok(Value::Table(t))
                    }
                    _ => Ok(Value::Nil),
                }
            }
        })?,
    )?;

    // ui.prompt(message) — show a text input prompt, returns string or nil
    let ctx_prompt = ctx.clone();
    ui.set(
        "prompt",
        lua.create_async_function(move |_lua, message: String| {
            let ctx = ctx_prompt.clone();
            async move {
                let resp = ctx
                    .send_command(PluginCommand::ShowPrompt { message })
                    .await;
                match resp {
                    PluginResponse::Output(s) => Ok(Value::String(
                        _lua.create_string(&s)?,
                    )),
                    _ => Ok(Value::Nil),
                }
            }
        })?,
    )?;

    // ui.confirm(message) — show a yes/no dialog, returns boolean
    let ctx_confirm = ctx.clone();
    ui.set(
        "confirm",
        lua.create_async_function(move |_lua, message: String| {
            let ctx = ctx_confirm.clone();
            async move {
                let resp = ctx
                    .send_command(PluginCommand::ShowConfirm { message })
                    .await;
                match resp {
                    PluginResponse::Bool(b) => Ok(Value::Boolean(b)),
                    _ => Ok(Value::Boolean(false)),
                }
            }
        })?,
    )?;

    // ui.alert(title, msg) — show an informational alert
    let ctx_alert = ctx.clone();
    ui.set(
        "alert",
        lua.create_async_function(move |_lua, (title, message): (String, String)| {
            let ctx = ctx_alert.clone();
            async move {
                let _ = ctx
                    .send_command(PluginCommand::ShowAlert { title, message })
                    .await;
                Ok(())
            }
        })?,
    )?;

    // ui.error(title, msg) — show an error alert
    let ctx_error = ctx.clone();
    ui.set(
        "error",
        lua.create_async_function(move |_lua, (title, message): (String, String)| {
            let ctx = ctx_error.clone();
            async move {
                let _ = ctx
                    .send_command(PluginCommand::ShowError { title, message })
                    .await;
                Ok(())
            }
        })?,
    )?;

    // ui.show(title, text) — show a read-only text viewer
    let ctx_show = ctx.clone();
    ui.set(
        "show",
        lua.create_async_function(move |_lua, (title, text): (String, String)| {
            let ctx = ctx_show.clone();
            async move {
                let _ = ctx
                    .send_command(PluginCommand::ShowText { title, text })
                    .await;
                Ok(())
            }
        })?,
    )?;

    // ui.table(title, columns, rows) — show a table viewer
    let ctx_table = ctx.clone();
    ui.set(
        "table",
        lua.create_async_function(
            move |_lua, (title, cols_table, rows_table): (String, mlua::Table, mlua::Table)| {
                let ctx = ctx_table.clone();
                async move {
                    let columns: Vec<String> = cols_table
                        .sequence_values::<String>()
                        .collect::<Result<_, _>>()?;
                    let mut rows = Vec::new();
                    for row_val in rows_table.sequence_values::<mlua::Table>() {
                        let row_table = row_val?;
                        let row: Vec<String> = row_table
                            .sequence_values::<String>()
                            .collect::<Result<_, _>>()?;
                        rows.push(row);
                    }
                    let _ = ctx
                        .send_command(PluginCommand::ShowTable {
                            title,
                            columns,
                            rows,
                        })
                        .await;
                    Ok(())
                }
            },
        )?,
    )?;

    // ui.progress(message) — show a progress spinner
    let ctx_progress = ctx.clone();
    ui.set(
        "progress",
        lua.create_async_function(move |_lua, message: String| {
            let ctx = ctx_progress.clone();
            async move {
                let _ = ctx
                    .send_command(PluginCommand::ShowProgress { message })
                    .await;
                Ok(())
            }
        })?,
    )?;

    // ui.hide_progress() — hide the progress spinner
    let ctx_hide = ctx.clone();
    ui.set(
        "hide_progress",
        lua.create_async_function(move |_lua, ()| {
            let ctx = ctx_hide.clone();
            async move {
                let _ = ctx
                    .send_command(PluginCommand::HideProgress)
                    .await;
                Ok(())
            }
        })?,
    )?;

    lua.globals().set("ui", ui)?;
    Ok(())
}

/// Parse a Lua table of field descriptors into `Vec<FormField>`.
///
/// Each sub-table has:
///   `type` = "text" | "password" | "combo" | "checkbox" | "separator" | "label"
///   `name` = field name (for text/password/combo/checkbox)
///   `label` = display label
///   `default` = default value (string for text/combo, bool for checkbox)
///   `options` = list of strings (for combo)
///   `text` = label text (for label type)
fn parse_form_fields(table: &mlua::Table) -> LuaResult<Vec<FormField>> {
    let mut fields = Vec::new();

    for entry in table.sequence_values::<mlua::Table>() {
        let entry = entry?;
        let field_type: String = entry.get("type")?;

        let field = match field_type.as_str() {
            "text" => FormField::Text {
                name: entry.get("name")?,
                label: entry.get("label").unwrap_or_default(),
                default: entry.get("default").unwrap_or_default(),
            },
            "password" => FormField::Password {
                name: entry.get("name")?,
                label: entry.get("label").unwrap_or_default(),
            },
            "combo" => {
                let opts_table: mlua::Table = entry.get("options")?;
                let options: Vec<String> = opts_table
                    .sequence_values::<String>()
                    .collect::<Result<_, _>>()?;
                FormField::ComboBox {
                    name: entry.get("name")?,
                    label: entry.get("label").unwrap_or_default(),
                    options,
                    default: entry.get("default").unwrap_or_default(),
                }
            }
            "checkbox" => FormField::CheckBox {
                name: entry.get("name")?,
                label: entry.get("label").unwrap_or_default(),
                default: entry.get("default").unwrap_or(false),
            },
            "separator" => FormField::Separator,
            "label" => FormField::Label {
                text: entry.get("text").unwrap_or_default(),
            },
            other => {
                return Err(mlua::Error::runtime(format!(
                    "Unknown form field type: '{other}'"
                )));
            }
        };

        fields.push(field);
    }

    Ok(fields)
}
