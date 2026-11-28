//! `autumn.http.fetch`: one HTTP request through `net`'s outbound guard
//! (#267 §2, ADR 0015).
//!
//! The function is async, so the VM lock is free while it waits. Dropping the
//! call (wall timeout, cancellation, stop) drops the request future, which
//! cancels the `reqwest` request.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use mlua::{Lua, LuaString, Table, Value};
use net::{ExtensionHosts, GuardedClient, Method, Request};

use super::{Builder, HostError, Kind};
use crate::limits::{Kill, KillSwitch};

/// The most requests one call may make; the next one kills the call.
pub(crate) const MAX_REQUESTS_PER_CALL: usize = 8;

/// The client and hosts an extension's requests go through.
#[derive(Debug, Clone)]
pub(crate) struct Network {
    /// The shared guarded client.
    pub(crate) client: GuardedClient,
    /// The extension's declared hosts, LAN exceptions and rate-limit key.
    pub(crate) hosts: ExtensionHosts,
}

/// One VM's HTTP state: the network and this call's request count.
#[derive(Debug)]
pub(crate) struct Http {
    network: Network,
    switch: Arc<KillSwitch>,
    requests: AtomicUsize,
    /// The message of the last `blocked` error this call raised, so an
    /// uncaught one can be reported as `NetworkBlocked`.
    blocked: Mutex<Option<String>>,
}

impl Http {
    pub(crate) fn new(network: Network, switch: Arc<KillSwitch>) -> Arc<Self> {
        Arc::new(Self {
            network,
            switch,
            requests: AtomicUsize::new(0),
            blocked: Mutex::new(None),
        })
    }

    /// Resets the per-call request count.
    pub(crate) fn begin_call(&self) {
        self.requests.store(0, Ordering::SeqCst);
        *self.lock_blocked() = None;
    }

    /// Whether `message` is the `blocked` error this call last raised.
    pub(crate) fn was_blocked(&self, message: &str) -> bool {
        self.lock_blocked().as_deref() == Some(message)
    }

    fn lock_blocked(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.blocked.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `autumn.http`.
    pub(super) fn module(self: &Arc<Self>, build: &Builder<'_>) -> mlua::Result<Table> {
        let http = Arc::clone(self);
        build.table([(
            "fetch",
            build.async_function(NAME, move |lua, request: Table| {
                let http = Arc::clone(&http);
                async move { http.fetch(&lua, &request).await }
            })?,
        )])
    }

    /// One request. An outer `Err` is the request-limit kill, which the
    /// wrapper raises as is; an inner `Err` is a catchable host error.
    async fn fetch(&self, lua: &Lua, request: &Table) -> mlua::Result<Result<Table, HostError>> {
        if self.requests.fetch_add(1, Ordering::SeqCst) >= MAX_REQUESTS_PER_CALL {
            self.switch.kill(Kill::Requests);
            return Err(mlua::Error::runtime(Kill::Requests.message()));
        }
        let request = match parse_request(lua, request) {
            Ok(request) => request,
            Err(error) => return Ok(Err(error)),
        };
        let outcome = self.send(lua, request).await;
        if let Err(error) = &outcome
            && error.kind == Kind::Blocked
        {
            *self.lock_blocked() = Some(error.message(NAME));
        }
        Ok(outcome)
    }

    async fn send(&self, lua: &Lua, request: Request) -> Result<Table, HostError> {
        let response = self
            .network
            .client
            .send(&self.network.hosts, request)
            .await
            .map_err(|error| net_error(&error))?;
        let table = lua.create_table()?;
        table.raw_set("status", response.status())?;
        table.raw_set("url", response.final_url())?;
        let mut headers: BTreeMap<&str, String> = BTreeMap::new();
        for (name, value) in response.headers() {
            headers
                .entry(name)
                .and_modify(|joined| {
                    joined.push_str(", ");
                    joined.push_str(value);
                })
                .or_insert_with(|| value.to_owned());
        }
        table.raw_set("headers", lua.create_table_from(headers)?)?;
        let cookies = lua.create_sequence_from(response.header_values("set-cookie"))?;
        table.raw_set("set_cookies", cookies)?;
        let body = response.bytes().await.map_err(|error| net_error(&error))?;
        table.raw_set("body", lua.create_string(body)?)?;
        Ok(table)
    }
}

const NAME: &str = "autumn.http.fetch";

/// Reads `{ url, method, headers, body, query }` into a guarded request.
fn parse_request(lua: &Lua, table: &Table) -> Result<Request, HostError> {
    let url: String =
        field(lua, table, "url", "string")?.ok_or_else(|| HostError::invalid("url: missing"))?;
    let method = match field::<String>(lua, table, "method", "string")?.as_deref() {
        None => Method::Get,
        Some(method) => match method.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            _ => {
                return Err(HostError::invalid(format!(
                    "method: {method:?} is not GET, POST, PUT, PATCH or DELETE"
                )));
            }
        },
    };
    url::Url::parse(&url).map_err(|error| HostError::invalid(format!("url: {error}")))?;
    let url = match field::<Table>(lua, table, "query", "table")? {
        // Errors name the field, `query.<key>`.
        Some(query) => super::codec::build_url(&url, &query)?,
        None => url,
    };
    let mut request = Request::new(method, url);
    if let Some(headers) = field::<Table>(lua, table, "headers", "table")? {
        for pair in headers.pairs::<String, String>() {
            let (name, value) = pair.map_err(|_| {
                HostError::invalid("headers: expected string names and string values")
            })?;
            request = request
                .header(&name, &value)
                .map_err(|error| HostError::invalid(format!("headers.{name}: {error}")))?;
        }
    }
    if let Some(body) = field::<LuaString>(lua, table, "body", "string")? {
        request = request.body(body.as_bytes().to_vec());
    }
    Ok(request)
}

/// An optional field of type `T`, or `invalid_argument` naming it.
fn field<T: mlua::FromLua>(
    lua: &Lua,
    table: &Table,
    name: &str,
    expected: &str,
) -> Result<Option<T>, HostError> {
    match table.raw_get::<Value>(name)? {
        Value::Nil => Ok(None),
        value => {
            let got = value.type_name();
            let wrong = || HostError::invalid(format!("{name}: expected {expected}, got {got}"));
            // mlua would turn a number into a string; a request field is strict.
            let strict = match expected {
                "string" => value.is_string(),
                "table" => value.is_table(),
                _ => true,
            };
            if !strict {
                return Err(wrong());
            }
            T::from_lua(value, lua).map(Some).map_err(|_| wrong())
        }
    }
}

/// The catchable kind for a `net` failure.
fn net_error(error: &net::Error) -> HostError {
    let kind = match error {
        net::Error::Blocked { .. } => Kind::Blocked,
        net::Error::RequestTooLarge { .. } | net::Error::ResponseTooLarge { .. } => Kind::TooLarge,
        net::Error::RateLimited { .. } => Kind::RateLimited,
        net::Error::InvalidRequest { .. } => Kind::InvalidArgument,
        error if error.is_timeout() => Kind::Timeout,
        _ => Kind::Network,
    };
    // `net` messages name the host but never the URL or query.
    let detail = error.to_string();
    let detail = detail
        .strip_prefix("blocked: ")
        .unwrap_or(&detail)
        .to_owned();
    HostError::new(kind, detail)
}
