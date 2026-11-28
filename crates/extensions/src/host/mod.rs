//! The `autumn` global: the host API a Luau extension calls (#267).
//!
//! [`install`] builds every module, freezes `autumn` and its sub-tables, and
//! sets the global; it runs before `sandbox(true)`. Each host function is a
//! Rust closure that returns `(true, value)` or `(false, message)`, wrapped
//! by [`RAISE`] so a failure reaches the script as a plain string error,
//! `"<function>: <kind>: <detail>"`, which it can `pcall` and match on.
//!
//! `autumn.http.fetch` (#305) and `autumn.settings.get` (#304) are added by
//! their own tickets; [`AUTUMN_D_LUAU`] already declares them.

mod codec;
mod html;
mod log;
mod time;

use mlua::{FromLuaMulti, Function, IntoLua, Lua, MultiValue, Table, Value};

pub(crate) use log::CallLog;

/// The typed definitions of the whole `autumn` global, for the playground and
/// editors.
pub const AUTUMN_D_LUAU: &str = include_str!("autumn.d.luau");

/// The manifest `api_version`s this host runs. Changes within a version are
/// additive only (#267 §6).
pub const SUPPORTED_API_VERSIONS: &[u32] = &[1];

/// Turns a `(ok, value)` host function into one that raises `value` as a
/// string when `ok` is false. Level 0 keeps a position prefix off the message.
const RAISE: &str = r"
local error = error
return function(host)
    return function(...)
        local ok, value = host(...)
        if not ok then
            error(value, 0)
        end
        return value
    end
end
";

/// A catchable host-call failure kind (#267 §3). `request_limit` is not one:
/// it only ever appears in a kill message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    InvalidArgument,
    #[expect(dead_code, reason = "raised by autumn.http.fetch (#305)")]
    Blocked,
    #[expect(dead_code, reason = "raised by autumn.http.fetch (#305)")]
    Network,
    #[expect(dead_code, reason = "raised by autumn.http.fetch (#305)")]
    Timeout,
    #[expect(dead_code, reason = "raised by autumn.http.fetch (#305)")]
    TooLarge,
    #[expect(dead_code, reason = "raised by autumn.http.fetch (#305)")]
    RateLimited,
    Parse,
    #[expect(dead_code, reason = "raised by autumn.settings.get (#304)")]
    Keychain,
}

impl Kind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::Blocked => "blocked",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::TooLarge => "too_large",
            Self::RateLimited => "rate_limited",
            Self::Parse => "parse",
            Self::Keychain => "keychain",
        }
    }
}

/// Why a host call failed, before it is named after its function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostError {
    kind: Kind,
    detail: String,
}

impl HostError {
    pub(crate) fn new(kind: Kind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Self::new(Kind::InvalidArgument, detail)
    }

    pub(crate) fn parse(detail: impl Into<String>) -> Self {
        Self::new(Kind::Parse, detail)
    }
}

impl From<mlua::Error> for HostError {
    /// A VM failure while building a result, reported as a bad argument
    /// since the script's input is what made it fail.
    fn from(error: mlua::Error) -> Self {
        Self::invalid(error.to_string())
    }
}

/// Builds host functions and the tables that hold them.
pub(crate) struct Builder<'lua> {
    lua: &'lua Lua,
    raise: Function,
}

impl<'lua> Builder<'lua> {
    fn new(lua: &'lua Lua) -> mlua::Result<Self> {
        let raise = lua.load(RAISE).set_name("=autumn").eval()?;
        Ok(Self { lua, raise })
    }

    /// A raising Luau function named `name` (as in `autumn.json.decode`)
    /// that runs `call` on its unpacked arguments. Arguments that don't
    /// convert are an `invalid_argument` error.
    pub(crate) fn function<A, R, F>(&self, name: &'static str, call: F) -> mlua::Result<Function>
    where
        A: FromLuaMulti,
        R: IntoLua,
        F: Fn(&Lua, A) -> Result<R, HostError> + Send + 'static,
    {
        let host = self.lua.create_function(move |lua, args: MultiValue| {
            let outcome = A::from_lua_multi(args, lua)
                .map_err(|error| HostError::invalid(argument_error(&error)))
                .and_then(|args| call(lua, args))
                .and_then(|value| value.into_lua(lua).map_err(HostError::from));
            Ok(match outcome {
                Ok(value) => (true, value),
                Err(error) => {
                    let message = format!("{name}: {}: {}", error.kind.as_str(), error.detail);
                    (false, Value::String(lua.create_string(message)?))
                }
            })
        })?;
        self.raise.call(host)
    }

    /// A table of `(key, function)` entries.
    pub(crate) fn table(
        &self,
        entries: impl IntoIterator<Item = (&'static str, Function)>,
    ) -> mlua::Result<Table> {
        let table = self.lua.create_table()?;
        for (key, function) in entries {
            table.raw_set(key, function)?;
        }
        Ok(table)
    }
}

/// mlua's conversion message, without its `error converting …` framing
/// where it has one.
fn argument_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::FromLuaConversionError { from, to, message } => match message {
            Some(message) => format!("expected {to}, got {from}: {message}"),
            None => format!("expected {to}, got {from}"),
        },
        mlua::Error::BadArgument { pos, cause, .. } => {
            format!("argument #{pos}: {}", argument_error(cause))
        }
        other => other.to_string(),
    }
}

/// Installs the frozen `autumn` global. Must run before `sandbox(true)`.
pub(crate) fn install(lua: &Lua, log: &CallLog) -> mlua::Result<()> {
    let build = Builder::new(lua)?;
    let autumn = lua.create_table()?;
    autumn.raw_set("html", html::module(&build)?)?;
    autumn.raw_set("json", codec::json(&build)?)?;
    autumn.raw_set("url", codec::url(&build)?)?;
    autumn.raw_set("base64", codec::base64(&build)?)?;
    autumn.raw_set("hex", codec::hex(&build)?)?;
    autumn.raw_set("crypto", codec::crypto(&build)?)?;
    autumn.raw_set("time", time::module(&build)?)?;
    autumn.raw_set("log", log.module(&build)?)?;
    freeze(&autumn)?;
    lua.globals().raw_set("print", log.print(&build)?)?;
    lua.globals().raw_set("autumn", autumn)
}

/// Makes `table` and every table inside it read-only.
fn freeze(table: &Table) -> mlua::Result<()> {
    for pair in table.pairs::<Value, Value>() {
        if let (_, Value::Table(inner)) = pair? {
            freeze(&inner)?;
        }
    }
    table.set_readonly(true);
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! A VM with just the host API, for per-function tests.

    use domain::ExtensionId;
    use mlua::{FromLuaMulti, Lua};

    use super::{CallLog, install};

    /// A sandboxed VM with `autumn` installed, and its call log.
    pub(crate) fn vm() -> (Lua, CallLog) {
        let lua = Lua::new();
        let log = CallLog::new(ExtensionId::new("t"));
        install(&lua, &log).unwrap();
        lua.sandbox(true).unwrap();
        (lua, log)
    }

    /// Evaluates `source` in a fresh VM.
    pub(crate) fn eval<R: FromLuaMulti>(source: &str) -> mlua::Result<R> {
        vm().0.load(source).eval()
    }

    /// The string a failing expression raises, from `pcall`.
    pub(crate) fn raised(expression: &str) -> String {
        let source = format!(
            "local ok, err = pcall(function() return {expression} end) assert(not ok, 'expected an error') return err"
        );
        eval(&source).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{eval, raised};

    #[test]
    fn autumn_and_its_modules_are_read_only() {
        let top = raised("(function() autumn.x = 1 end)()");
        assert!(top.contains("readonly"), "{top}");
        let module = raised("(function() autumn.json.x = 1 end)()");
        assert!(module.contains("readonly"), "{module}");
        let replaced = raised("(function() autumn.json.decode = print end)()");
        assert!(replaced.contains("readonly"), "{replaced}");
    }

    #[test]
    fn bad_arguments_raise_invalid_argument_strings() {
        let error = raised("autumn.hex.encode({})");
        assert!(
            error.starts_with("autumn.hex.encode: invalid_argument: "),
            "{error}"
        );
        let kind: String =
            eval("local ok, err = pcall(autumn.hex.encode, {}) return typeof(err)").unwrap();
        assert_eq!(kind, "string", "scripts can match on the message");
    }

    #[test]
    fn every_installed_function_is_declared_in_the_definitions() {
        let names: Vec<String> = eval(
            r#"
            local names = {}
            for module, functions in autumn do
                for name in functions do
                    table.insert(names, module .. "." .. name)
                end
            end
            return names
            "#,
        )
        .unwrap();
        assert!(names.len() > 15, "{names:?}");
        for name in names {
            let (module, function) = name.split_once('.').unwrap();
            assert!(
                super::AUTUMN_D_LUAU.contains(&format!("{module}: {{"))
                    && super::AUTUMN_D_LUAU.contains(&format!("{function}: (")),
                "{name} is missing from autumn.d.luau"
            );
        }
    }
}
