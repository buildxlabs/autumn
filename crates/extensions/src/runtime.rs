//! One sandboxed Luau VM running one torrent provider.
//!
//! The VM is built and loaded in [`Vm::load`] and called through
//! [`Vm::search`]. Both run the script under a host `xpcall` wrapper, so an
//! uncaught error comes back with its message and `debug.traceback` instead
//! of mlua's own formatting. Callers serialise access through the actor in
//! [`crate::actor`] (ADR 0014), which also enforces [`crate::limits`].

use std::sync::Arc;

use domain::ExtensionId;
use mlua::{Function, Lua, Table, Value, VmState};

use crate::{
    error::ExtensionError,
    host::{self, CallLog},
    limits::{Kill, KillSwitch},
    provider::{ProviderRequest, ProviderTorrent},
};

/// Luau's error value for a failed allocation.
const OUT_OF_MEMORY: &str = "not enough memory";

/// Prepended to the extension's source on its first line, so line numbers
/// don't move. Luau compiles a call through the *global* `pcall` or `xpcall`
/// to a fast path that runs the VM's builtin and skips [`KILL_GUARDS`];
/// through a local it makes an ordinary call to the guarded function.
const GUARD_PRELUDE: &str = "local pcall, xpcall = pcall, xpcall; ";

/// Wraps `pcall`, `xpcall` and `coroutine.resume` so a kill the host decided
/// on is raised again instead of caught, and a caught out-of-memory error
/// still counts as a memory kill (#267 §3). Runs with `is_killed` as its
/// argument, before `sandbox(true)` freezes the builtins.
const KILL_GUARDS: &str = r"
local is_killed = ...
local error, raw_pcall, raw_xpcall, raw_resume = error, pcall, xpcall, coroutine.resume
local function pass(ok, ...)
    if not ok and is_killed((...)) then
        error((...), 0)
    end
    return ok, ...
end
pcall = function(...)
    return pass(raw_pcall(...))
end
xpcall = function(fn, handler, ...)
    return pass(raw_xpcall(fn, function(err)
        if is_killed(err) then
            return err
        end
        return handler(err)
    end, ...))
end
coroutine.resume = function(...)
    return pass(raw_resume(...))
end
";

/// Runs `fn(arg)` under `xpcall` and returns `true, result` or
/// `false, { message, traceback }`. Builtins are captured as upvalues while
/// the VM is still writable, so nothing a script does can swap them out.
const PROTECTED_CALL: &str = r#"
local xpcall, tostring, type, traceback = xpcall, tostring, type, debug.traceback
return function(fn, arg)
    return xpcall(fn, function(err)
        local message
        if type(err) == "table" and type(err.message) == "string" then
            message = err.message
        else
            message = tostring(err)
        end
        return { message = message, traceback = traceback(nil, 2) }
    end, arg)
end
"#;

/// Globals removed before `sandbox(true)` (#267 §4). mlua's `require` reads
/// the filesystem and survives the sandbox; the other three complicate
/// isolation or compile code at run time.
const REMOVED_GLOBALS: [&str; 4] = ["require", "getfenv", "setfenv", "loadstring"];

/// Installs extra globals before `sandbox(true)`; tests use it for host
/// functions the host API doesn't offer.
pub(crate) type Prepare = fn(&Lua) -> mlua::Result<()>;

/// A loaded torrent provider and the VM it lives in.
#[derive(Debug)]
pub(crate) struct Vm {
    id: ExtensionId,
    lua: Lua,
    protected_call: Function,
    search: Function,
    switch: Arc<KillSwitch>,
    log: CallLog,
}

impl Vm {
    /// Builds a sandboxed VM capped at `memory` bytes, wires its interrupt to
    /// `switch`, and runs the provider chunk in it.
    ///
    /// # Errors
    ///
    /// - [`ExtensionError::Runtime`] when the chunk fails to compile or raises.
    /// - [`ExtensionError::InvalidProvider`] when it doesn't return a table
    ///   with a function `search`.
    /// - [`ExtensionError::Vm`] when preparing the VM fails.
    pub(crate) async fn load(
        id: ExtensionId,
        source: &str,
        memory: usize,
        switch: Arc<KillSwitch>,
        prepare: Prepare,
    ) -> Result<Self, ExtensionError> {
        let lua = Lua::new();
        let globals = lua.globals();
        for name in REMOVED_GLOBALS {
            globals.raw_set(name, Value::Nil)?;
        }
        let log = CallLog::new(id.clone());
        host::install(&lua, &log)?;
        lua.load(KILL_GUARDS)
            .set_name("=autumn")
            .call::<()>(is_killed(&lua, &switch)?)?;
        let protected_call: Function = lua.load(PROTECTED_CALL).set_name("=autumn").eval()?;
        prepare(&lua)?;
        lua.sandbox(true)?;
        let interrupt_switch = Arc::clone(&switch);
        switch.attach(lua.set_interrupt_oneshot(move |_| {
            match interrupt_switch.reason() {
                Some(kill) => {
                    // Raise again at the next safepoint until the call unwinds.
                    interrupt_switch.rearm();
                    Err(mlua::Error::runtime(kill.message()))
                }
                None => Ok(VmState::Continue),
            }
        }));
        lua.set_memory_limit(memory)?;

        let chunk = lua
            .load(format!("{GUARD_PRELUDE}{source}"))
            .set_name(format!("={id}"))
            .into_function();
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(mlua::Error::SyntaxError { message, .. }) => {
                return Err(ExtensionError::runtime(message, String::new()));
            }
            Err(error) => return Err(error.into()),
        };
        let call = log.begin_call();
        let provider = protected(&protected_call, chunk, Value::Nil).await?;
        drop(call);
        let search = provider_search(provider)?;
        Ok(Self {
            id,
            lua,
            protected_call,
            search,
            switch,
            log,
        })
    }

    /// The switch that kills this VM's calls.
    pub(crate) fn switch(&self) -> Arc<KillSwitch> {
        Arc::clone(&self.switch)
    }

    /// Calls the provider's `search(request)` and decodes its rows.
    ///
    /// # Errors
    ///
    /// - [`ExtensionError::Runtime`] when the script raises.
    /// - [`ExtensionError::InvalidResult`] when it doesn't return an array.
    /// - [`ExtensionError::Vm`] when encoding the request fails.
    pub(crate) async fn search(
        &self,
        request: &ProviderRequest,
    ) -> Result<Vec<ProviderTorrent>, ExtensionError> {
        let request = encode_request(&self.lua, request)?;
        let _call = self.log.begin_call();
        let results = protected(
            &self.protected_call,
            self.search.clone(),
            Value::Table(request),
        )
        .await?;
        decode_results(&self.id, results)
    }
}

/// The guards' `is_killed(err)`: whether the host killed the call, counting
/// an out-of-memory error as a memory kill.
fn is_killed(lua: &Lua, switch: &Arc<KillSwitch>) -> mlua::Result<Function> {
    let switch = Arc::clone(switch);
    lua.create_function(move |_, err: Value| {
        let out_of_memory = match &err {
            Value::String(err) => err.as_bytes().as_ref() == OUT_OF_MEMORY.as_bytes(),
            Value::Error(err) => matches!(**err, mlua::Error::MemoryError(_)),
            _ => false,
        };
        if out_of_memory {
            switch.kill(Kill::Memory);
        }
        Ok(switch.reason().is_some())
    })
}

/// Runs `function(arg)` through the `xpcall` wrapper.
async fn protected(
    protected_call: &Function,
    function: Function,
    arg: Value,
) -> Result<Value, ExtensionError> {
    let (ok, value): (bool, Value) = protected_call.call_async((function, arg)).await?;
    if ok {
        return Ok(value);
    }
    let (message, traceback) = match value {
        Value::Table(error) => (
            error
                .raw_get::<Option<String>>("message")?
                .unwrap_or_default(),
            error
                .raw_get::<Option<String>>("traceback")?
                .unwrap_or_default(),
        ),
        // The handler itself failed, so `xpcall` returned its own message.
        other => (other.to_string()?, String::new()),
    };
    Err(ExtensionError::runtime(message, traceback))
}

/// Checks the chunk's return is `{ search = function }` (#268 Q1).
fn provider_search(provider: Value) -> Result<Function, ExtensionError> {
    let Value::Table(provider) = provider else {
        return Err(ExtensionError::InvalidProvider(format!(
            "provider: expected table, got {}",
            provider.type_name()
        )));
    };
    match provider.raw_get::<Value>("search")? {
        Value::Function(search) => Ok(search),
        other => Err(ExtensionError::InvalidProvider(format!(
            "search: expected function, got {}",
            other.type_name()
        ))),
    }
}

/// A fresh, unfrozen request table; absent fields stay `nil`.
fn encode_request(lua: &Lua, request: &ProviderRequest) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("keyword", request.keyword.as_str())?;
    if let Some(episode) = &request.episode {
        table.raw_set(
            "titles",
            lua.create_sequence_from(episode.titles.iter().map(String::as_str))?,
        )?;
        table.raw_set("season", episode.season)?;
        table.raw_set("episode", episode.episode)?;
    }
    table.raw_set("absolute_episode", request.absolute_episode)?;
    table.raw_set("anidb_aid", request.anidb_aid)?;
    table.raw_set("anidb_eid", request.anidb_eid)?;
    Ok(table)
}

/// Checks `results` is a sequence with keys exactly `1..n` and decodes each
/// row (#268 Q4 step 1).
fn decode_results(
    id: &ExtensionId,
    results: Value,
) -> Result<Vec<ProviderTorrent>, ExtensionError> {
    let Value::Table(results) = results else {
        return Err(ExtensionError::InvalidResult(format!(
            "results: expected array, got {}",
            results.type_name()
        )));
    };
    let mut len = 0_usize;
    let mut keys = Vec::new();
    for pair in results.pairs::<Value, Value>() {
        let (key, _) = pair?;
        keys.push(key);
        len += 1;
    }
    if let Some(key) = keys.iter().find(|key| !is_sequence_key(key, len)) {
        return Err(ExtensionError::InvalidResult(format!(
            "results: expected array, got table with key {}",
            describe(key)
        )));
    }
    let mut rows = Vec::with_capacity(len);
    for index in 1..=len {
        let row = results.raw_get::<Value>(index)?;
        rows.push(decode_row(id, index, &row)?);
    }
    Ok(rows)
}

/// Whether `key` is a whole number in `1..=len`.
fn is_sequence_key(key: &Value, len: usize) -> bool {
    match key {
        Value::Integer(index) => {
            usize::try_from(*index).is_ok_and(|index| (1..=len).contains(&index))
        }
        Value::Number(index) => {
            let len = f64::from(u32::try_from(len).unwrap_or(u32::MAX));
            index.fract() == 0.0 && (1.0..=len).contains(index)
        }
        _ => false,
    }
}

/// A key as it would read in Luau source, for error paths.
fn describe(key: &Value) -> String {
    match key {
        Value::String(key) => format!("{:?}", key.to_string_lossy()),
        other => other
            .to_string()
            .unwrap_or_else(|_| other.type_name().to_owned()),
    }
}

/// Decodes one row; a non-table row decodes to an empty one, which `search`
/// drops for having no name.
fn decode_row(id: &ExtensionId, index: usize, row: &Value) -> mlua::Result<ProviderTorrent> {
    let Value::Table(row) = row else {
        tracing::debug!(
            target: "extension",
            extension = %id,
            "results[{index}]: expected table, got {}",
            row.type_name()
        );
        return Ok(ProviderTorrent::default());
    };
    let field = Field { id, index, row };
    Ok(ProviderTorrent {
        name: field.string("name")?,
        infohash: field.string("infohash")?,
        magnet_url: field.string("magnet_url")?,
        torrent_url: field.string("torrent_url")?,
        size: field.number("size")?,
        seeders: field.number("seeders")?,
        leechers: field.number("leechers")?,
        downloads: field.number("downloads")?,
        published: field.number("published")?,
        is_batch: field.boolean("is_batch")?,
        anidb_aid: field.number("anidb_aid")?,
        anidb_eid: field.number("anidb_eid")?,
        num_files: field.number("num_files")?,
        files: field.strings("files")?,
    })
}

/// Reads typed fields from one result row, clearing wrong types.
struct Field<'row> {
    id: &'row ExtensionId,
    index: usize,
    row: &'row Table,
}

impl Field<'_> {
    fn string(&self, name: &str) -> mlua::Result<Option<String>> {
        Ok(match self.row.raw_get::<Value>(name)? {
            Value::Nil => None,
            Value::String(value) => Some(value.to_string_lossy()),
            other => self.wrong_type(name, "string", &other),
        })
    }

    #[expect(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        reason = "mlua reports a Luau double as Integer only when it converts back exactly"
    )]
    fn number(&self, name: &str) -> mlua::Result<Option<f64>> {
        Ok(match self.row.raw_get::<Value>(name)? {
            Value::Nil => None,
            Value::Number(value) => Some(value),
            Value::Integer(value) => Some(value as f64),
            other => self.wrong_type(name, "number", &other),
        })
    }

    fn boolean(&self, name: &str) -> mlua::Result<Option<bool>> {
        Ok(match self.row.raw_get::<Value>(name)? {
            Value::Nil => None,
            Value::Boolean(value) => Some(value),
            other => self.wrong_type(name, "boolean", &other),
        })
    }

    fn strings(&self, name: &str) -> mlua::Result<Option<Vec<String>>> {
        Ok(match self.row.raw_get::<Value>(name)? {
            Value::Nil => None,
            Value::Table(list) => Some(
                list.sequence_values::<Value>()
                    .filter_map(|value| match value {
                        Ok(Value::String(value)) => Some(value.to_string_lossy()),
                        _ => None,
                    })
                    .collect(),
            ),
            other => self.wrong_type(name, "array of strings", &other),
        })
    }

    fn wrong_type<T>(&self, name: &str, expected: &str, got: &Value) -> Option<T> {
        tracing::debug!(
            target: "extension",
            extension = %self.id,
            "results[{}].{name}: expected {expected}, got {}",
            self.index,
            got.type_name()
        );
        None
    }
}
