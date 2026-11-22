# mlua Luau sandbox for extensions

Research for [#11](https://github.com/buildxlabs/autumn/issues/11). It checks
the extension-runtime decision in [#4](https://github.com/buildxlabs/autumn/issues/4):
Luau through `mlua` is the primary runtime, and WASM comes later.

Researched 2026-11-21 against **mlua 0.12.2** (released 2026-10-03, Luau 0.740)
and rustc 1.100.0 on macOS aarch64.

- `src/…` paths point into the mlua 0.12.2 crate source
  ([GitHub](https://github.com/mlua-rs/mlua), tag `v0.12.2`).
- "Ran" marks results from the throwaway test crate described in
  [What was run](#what-was-run). That crate was not committed.

## Verdict

**Yes, with caveats.** Luau's sandbox via mlua covers what Autumn needs:

- no file, process or env access
- read-only builtins
- a memory cap per VM
- an interrupt that kills `while true do end`, including under async on a
  multi-threaded Tokio runtime
- async host functions
- MIT licenses that cargo-deny already allows

The caveats are things Autumn has to do itself, because mlua does not do them:

1. **mlua installs a filesystem `require` by default**, and `sandbox(true)`
   leaves it in place. Remove it, or replace it with a custom `Require`,
   _before_ calling `sandbox(true)` (see Q1).
2. **Use one `Lua` per extension.** A sandbox isolates globals per thread, not
   per extension. Luau itself says separate VMs are the only guaranteed
   isolation.
3. **The time limit is our job.** Write an interrupt callback that checks a
   deadline or a budget. After an OOM, also run `gc_collect()` (or drop the VM).
4. **`vendored` does nothing for Luau.** It only adds a pointless `lua-src`
   build. Use `features = ["luau", "async", "send"]`.

## Q1. What does `Lua::sandbox(true)` lock down?

**What it does.** `Lua::sandbox(true)` calls `luaL_sandbox(state, 1)` and then
`luaL_sandboxthread(state)` (`src/state.rs:680`). Its rustdoc lists the effects:

- read-only libraries
- read-only builtin metatables
- read-only globals, with `safeenv` turned on
- a local environment table that "performs writes locally and proxies reads
  to the global environment"
- `collectgarbage` limited to `count`

**What Luau's own sandbox removes or restricts** (per <https://luau.org/sandbox>):

- removes `io`, `package`, `dofile`, `loadfile`, `load` and `string.dump`
- reduces `os` to `clock`, `date`, `difftime` and `time`
- reduces `debug` to `info` and `traceback`
- makes `loadstring` reject bytecode
- keeps `getfenv` and `setfenv`, which it says "complicate isolation"
- states that one VM is not full isolation: "load trusted and untrusted code
  into separate VMs"

**Ran, on a sandboxed VM:**

| Probe                                                                              | Result                                 |
| ---------------------------------------------------------------------------------- | -------------------------------------- |
| `io`, `load`, `dofile`, `loadfile`, `package`                                      | `nil`                                  |
| keys of `os`                                                                       | `clock`, `date`, `difftime`, `time`    |
| keys of `debug`                                                                    | `info`, `traceback`                    |
| `io.open`, `os.execute`, `os.getenv`                                               | error: nil value                       |
| `collectgarbage('collect')`                                                        | error: invalid option                  |
| `collectgarbage('count')`                                                          | works                                  |
| `string.len = nil`                                                                 | `attempt to modify a readonly table`   |
| `rawset(math, …)`                                                                  | `attempt to modify a readonly table`   |
| writing to the string metatable                                                    | `attempt to modify a readonly table`   |
| reaching the base globals via `getmetatable(getfenv(1)).__index` and writing to it | readonly error                         |
| `loadstring`                                                                       | still present and works on source text |

### Filesystem access left: `require`

mlua's Luau setup always sets `require` to `create_require_function(FsRequirer::new())`
(`src/luau/mod.rs:109-110`). The FS requirer resolves paths against
`env::current_dir()` and reads files with `std::fs`
(`src/luau/require/fs.rs`, named `TextRequirer` in 0.11, see
[CHANGELOG](https://github.com/mlua-rs/mlua/blob/main/CHANGELOG.md) 0.12.0-rc.1).
`sandbox(true)` does not remove it.

**Ran:**

- A sandboxed script did `require('./evil')` on a `.luau` file in the current
  directory. **It loaded and returned the file's table.**
- `require('./secret')` on a non-Luau text file failed with a syntax error.
  The error named the file but not its contents.
- With the host chunk name `=ext`, `require` failed with "not supported in
  this context".
- `globals().raw_set("require", nil)` _after_ `sandbox(true)` **did not help.**
  It only clears the proxy, and reads fall through to the base globals, so
  `require` still worked.
- `globals().raw_set("require", nil)` _before_ `sandbox(true)` worked:
  "attempt to call a nil value".

**What Autumn does:** remove `require` before sandboxing. If extensions need
modules, install our own `Require` impl over the extension's bundled files
(`Lua::create_require_function`, `src/luau/mod.rs:42`; the `Require` trait is
in `src/luau/require.rs:61`). Also consider removing `getfenv`, `setfenv` and
`loadstring` the same way: Luau suggests disabling the first two.

### Freezing globals per extension

- Builtins are frozen VM-wide by `sandbox(true)`.
- Each sandboxed thread writes to its own proxy table. **Ran:** `leak = 42`
  stayed visible to later chunks in the same VM and in `lua.globals()`.
- The proxy is per VM thread, not per extension. So one VM per extension is
  the isolation unit.
- Inside a VM, `Chunk::set_environment(table)` with
  `__index = globals` gives a chunk its own env. **Ran:** writes landed in the
  custom table.
- The host can expose a frozen per-extension API table through
  `Table::set_readonly` (Luau only, `src/table.rs:750`).

## Q2. Memory and CPU limits, and do they work under async?

### Memory

`Lua::set_memory_limit(bytes)`: "Once an allocation occurs that would pass
this memory limit, a `Error::MemoryError` is generated instead"
(`src/state.rs:1116`).

The limit is enforced by mlua's allocator (`MemoryState`). Luau itself
imposes none by default: "By default, no memory limits are imposed"
(<https://luau.org/sandbox>).

**Ran**, with an 8 MiB limit:

- A table-growing loop ended with `memory error: not enough memory`.
- `#string.rep('x', 1e9)` failed the same way.
- Straight after the OOM, even `return 1+1` failed with `MemoryError`, because
  the garbage was still live (used memory was 8.39 MB).
- After `lua.gc_collect()`, the VM worked again (0.42 MB used).

So the host should collect, or drop the VM, after an OOM.

### CPU / time

`Lua::set_interrupt(Fn(&Lua) -> Result<VmState>)` (`src/state.rs:843`, Luau
only):

- Returning `Err` aborts the script.
- Returning `VmState::Yield` yields, but only "at yieldable points", which
  0.11.3 fixed (CHANGELOG entry for #632).
- GC interrupts are ignored.
- Luau calls the interrupt "eventually" at function calls and loop iterations.
  A single long C call can delay it (<https://luau.org/sandbox>).

**Ran:**

- `while true do end` with a 200 ms deadline in the interrupt was killed with
  `runtime error: timeout` after **200.1 ms**.
- The same test under `exec_async().await` inside `tokio::spawn` on the
  multi-threaded runtime, after an `await` on an async host function, was
  killed after **200.1 ms**.
- An interrupt that returned `Yield` every 100 ticks and `Err` after 2000 ticks
  under `exec_async` stopped at exactly 2002 interrupts. So yields and a
  budget-based kill both work under async.
- A pathological pattern,
  `string.find(string.rep('a',24), string.rep('a*',24)..'b')`, was also cut at
  100 ms. In this Luau build the interrupt reached code inside the pattern
  matcher. Long host functions we write still need their own timeouts.

**Under async:** the interrupt runs on whichever worker thread is polling the
VM, so a wall-clock deadline (`Instant`) works. Time spent awaiting a host
future is not Luau execution, so the interrupt does not fire then. Wrap host
futures in `tokio::time::timeout`.

## Q3. Async: host async functions, `Send`, one VM per extension

**Async host functions.** `Lua::create_async_function`
(`src/state.rs:1603`) turns a Rust future into a Lua function.

- Scripts call it like a sync function, and the call suspends the coroutine.
- The script must be driven with `call_async`, `exec_async` or `eval_async`.
- "Any executor can be used" (README, <https://github.com/mlua-rs/mlua>).
- mlua's own examples include `async_http_reqwest` (`Cargo.toml` `[[example]]`).
- Dropping the `AsyncThread` ends the Rust future (CHANGELOG 0.11.0-beta.2).

**Ran:** a `fetch(url)` that slept 50 ms on Tokio returned
`"body of https://example.org"` to the script. This is the shape for
calling our `net` client.

**`send`.** "`mlua::Lua` implements `Send + Sync`", and the feature adds a
`Send` bound to functions and `Send + Sync` to `UserData` (README).

- Internally, `Lua` wraps `XRc<ReentrantMutex<RawLua>>` (`src/state.rs:58`), so
  calls into one VM are serialised.
- **Ran:** `fn assert_send::<Lua>() where Lua: Send + Sync` compiled.
- **Ran:** a cloned `Lua` moved into `tokio::spawn` ran `exec_async` to
  completion.

**One VM per extension** fits the lock: VMs never contend with each other.
Luau recommends separate VMs for isolation, and per-VM memory limits and
interrupts become per-extension limits for free. It is also the only way to
get a memory cap per extension, because the limit belongs to the state.

## Q4. Build and binary size

**Luau is always built from source.** The `luau` feature maps to `luau0-src`
(`mlua-sys` `Cargo.toml`, `luau = ["luau0-src"]`). The build script uses the
vendored path whenever `luau` is on (`build/main_inner.rs:4`). The README
says "`luau`: enable Luau support (auto vendored mode)".

**`vendored` is redundant for Luau.** It enables the `lua-src` and
`luajit-src` build dependencies. The build compiled `lua-src v551.0.2` that
the Luau build never uses. Drop it.

**MSRV:** 1.88 (`rust-version` in mlua 0.12.2 `Cargo.toml`). 0.11.6 needed 1.85.

**Ran on macOS aarch64, rustc 1.100.0:** mlua 0.12.2 with
`luau,vendored,async,send` built cleanly in a release build. The full build
from scratch took about 30 s. There were no warnings from mlua.

**Not tested here:** Linux and Windows. The README says mlua "is tested on
Windows/macOS/Linux … in GitHub Actions". `luau` plus `module` mode is
unsupported on Windows (`build/main_inner.rs:14`), but Autumn does not need
module mode. Phase 4 CI should build on all three.

**Binary size, ran:** release builds, stripped with `/usr/bin/strip`, no LTO:

| Binary                                             | Size        |
| -------------------------------------------------- | ----------- |
| Tokio-only baseline                                | 569,592 B   |
| Test binary with mlua + Luau (VM, compiler, async) | 2,294,904 B |

The delta is **about 1.7 MB**. With 0.11.6 it was about 1.45 MB.

## Q5. Licenses (cargo-deny)

| Crate                            | License | Source                                                 |
| -------------------------------- | ------- | ------------------------------------------------------ |
| `mlua` 0.12.2                    | MIT     | `Cargo.toml`; README: "licensed under the MIT license" |
| `mlua-sys` 0.13.0                | MIT     | `Cargo.toml`                                           |
| `luau0-src` 0.22.0+luau740       | MIT     | `Cargo.toml`                                           |
| `lua-src` (only with `vendored`) | MIT     | `Cargo.toml`                                           |

The `luau0-src` LICENSE carries the upstream notices: "Copyright (c)
2019-2024 Roblox Corporation" and "Copyright (c) 1994–2019 Lua.org, PUC-Rio".

MIT is already in `deny.toml` `[licenses] allow`, so no policy change is
needed.

## Q6. WASM later: wasmtime vs Extism

### wasmtime 51

**Fuel** (<https://docs.wasmtime.dev/examples-interrupting-wasm.html>):

- Set up with `Config::consume_fuel(true)` and `Store::set_fuel(n)`.
- It is deterministic: the same program with the same fuel stops at the same
  point (except under copy-on-write memory init).
- It costs more at runtime than epochs.

**Epochs** (same page):

- Set up with `Config::epoch_interruption(true)`, `Store::set_epoch_deadline`,
  and `Engine::increment_epoch()` called from any thread.
- It works on wall time and is not deterministic.
- The page measures "around a 10% slowdown".

**On expiry, both can either trap or yield to an async caller**:

- `Store::fuel_async_yield_interval`
- `Store::epoch_deadline_async_yield_and_update`
- `Store::epoch_deadline_callback`

(<https://docs.rs/wasmtime/latest/wasmtime/struct.Store.html>)

**Memory:** `Store::limiter` sets a `ResourceLimiter` (same page).

**Host functions:** registered through `Linker`, with async support. This is
lower-level: we design the ABI ourselves.

### Extism

**Host functions** (<https://extism.org/docs/concepts/host-functions>):

- They are WASM imports with optional typed I/O and user data.
- In Rust, user data must be `Send + Sync`.
- The page does not say whether they can be async.

**Manifest** (<https://extism.org/docs/concepts/manifest>):

- `memory.max_pages` (64 KiB pages)
- `memory.max_http_response_bytes`
- `memory.max_var_bytes`
- `allowed_hosts`: empty means no hosts, `null` means all
- `allowed_paths`, which needs WASI
- The manifest page does not mention fuel or a timeout.

**Tradeoff:** Extism gives a ready-made plugin ABI, PDKs in many languages,
and declarative host/path allow-lists. wasmtime gives exact fuel/epoch control
and native async. This is enough to defer the choice to the WASM phase; it was
not researched further.

## What was run

The throwaway crate lives in the session scratchpad (not committed). It
depends on:

```toml
mlua = { version = "0.12", features = [
  "luau",
  "vendored",
  "async",
  "send",
] } # resolved 0.12.2, luau0-src 0.22.0+luau740
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

It has three binaries, built with `cargo build --release` on rustc 1.100.0,
macOS aarch64:

1. **`main`**
   - lists globals, `os` keys and `debug` keys under the sandbox
   - probes `io`, `os` and `collectgarbage`
   - tries to write to read-only tables
   - checks where global writes land
   - tests the 8 MiB memory limit
   - runs the sync interrupt test
   - runs an async host function
   - runs the interrupt test under `exec_async` inside `tokio::spawn`
   - runs the yield-plus-budget interrupt test
2. **`edge`**
   - `require` of a `.luau` file and of a text file from the current directory
   - `require` with a host chunk name
   - removing `require` after vs before `sandbox(true)`
   - recovery after OOM with `gc_collect`
   - `string.rep(1e9)` against the limit
   - the interrupt during a pathological pattern match
   - trying to escape via the env metatable
3. **`baseline`**: Tokio only, for the size comparison.

The same tests were first run on mlua 0.11.6 (Luau 0.709) with identical
results.
