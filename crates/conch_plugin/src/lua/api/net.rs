//! `net.*` Lua table — time, DNS resolution, port scanning.

use mlua::prelude::*;

// ---------------------------------------------------------------------------
// net.* table
// ---------------------------------------------------------------------------

pub(super) fn register_net_table(lua: &Lua) -> LuaResult<()> {
    let net = lua.create_table()?;

    net.set(
        "time",
        lua.create_function(|_lua, ()| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            Ok(secs)
        })?,
    )?;

    net.set(
        "resolve",
        lua.create_function(|_lua, host: String| -> LuaResult<Vec<String>> {
            use std::net::ToSocketAddrs;
            let addr = format!("{host}:0");
            match addr.to_socket_addrs() {
                Ok(addrs) => Ok(addrs.map(|a| a.ip().to_string()).collect()),
                Err(_) => Ok(vec![]),
            }
        })?,
    )?;

    net.set(
        "scan",
        lua.create_function(
            |_lua,
             (host, ports, timeout_ms, _concurrency): (
                String,
                Vec<u16>,
                Option<u64>,
                Option<u32>,
            )|
             -> LuaResult<Vec<LuaTable>> {
                use std::net::{TcpStream, ToSocketAddrs};
                use std::time::Duration;

                let timeout = Duration::from_millis(timeout_ms.unwrap_or(1000));
                let mut results = Vec::new();

                for port in ports {
                    let addr_str = format!("{host}:{port}");
                    let open = match addr_str.to_socket_addrs() {
                        Ok(mut addrs) => {
                            if let Some(addr) = addrs.next() {
                                TcpStream::connect_timeout(&addr, timeout).is_ok()
                            } else {
                                false
                            }
                        }
                        Err(_) => false,
                    };
                    if open {
                        let tbl = _lua.create_table()?;
                        tbl.set("port", port)?;
                        tbl.set("open", true)?;
                        results.push(tbl);
                    }
                }

                Ok(results)
            },
        )?,
    )?;

    lua.globals().set("net", net)?;
    Ok(())
}
