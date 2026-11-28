# mlua Luau runtime control for extensions

Research for [#262](https://github.com/buildxlabs/autumn/issues/262). It builds on
[`mlua-luau-sandbox.md`](mlua-luau-sandbox.md), which proved that an interrupt kills
`while true do end`. This note covers what that spike left open: how the host stops a
call from outside, what a second caller sees, what a VM costs, and what crosses the
`send` boundary.

Researched 2026-11-28 against **mlua 0.12.3** (crate `mlua-sys 0.14.0`,
`luau0-src 0.24.1+luau748`) and rustc 1.100.0 on macOS aarch64, with
`features = ["luau", "async", "send"]`. The earlier spike used 0.12.2 / Luau 0.740;
two changes between them matter here: 0.12.2 fixed "async thread cancellation, stack
cleanup, and global hook restoration when recycling threads", and 0.12.3 added
`Lua::set_interrupt_oneshot` (CHANGELOG.md, v0.12.2 and v0.12.3 entries).

- `src/…` paths point into the mlua 0.12.3 crate source
  ([GitHub](https://github.com/mlua-rs/mlua), tag `v0.12.3`).
- "Ran" marks results from the throwaway crate in
  [What was run](#what-was-run). That crate was not committed.

## Verdict

**The interrupt is the only reliable stop button. Dropping a future is not.**
mlua 0.12.3 has a first-class cross-thread kill switch: `set_interrupt_oneshot`
returns an `InterruptHandle` whose `arm()` a watchdog thread can call while a call is
running; the callback fires on the Luau-executing thread at the next safepoint. The
older pattern — a `set_interrupt` callback reading an `AtomicBool` — also works and is
the only option on 0.12.2.

Everything else follows from one fact: `Lua` is an `Arc<ReentrantMutex<RawLua>>` under
`send`, and **every public method takes that lock synchronously**. A second call into
a VM that is looping blocks the caller (a Tokio worker or the UI thread) for as long
as the loop runs; a VM suspended at a host future does not hold the lock. Dropping a
future that is suspended at a host future cancels the Rust future and leaves the VM
usable; dropping one whose call is mid-loop does not preempt the loop at all.

After a kill or OOM the VM is reusable, but rebuilding it is cheap enough (roughly
0.1–0.8 ms plus loading the chunk) to be the simpler default when we want a clean
slate. One VM per extension remains the isolation and locking unit.

## Q1. Cancelling from outside, and the state of the VM afterwards

### The interrupt is the stop button

`Lua::set_interrupt(Fn(&Lua) -> Result<VmState>)` (Luau only, `src/state.rs:852`)
installs a callback that Luau invokes at safepoints. Returning `Err` aborts the script;
`VmState::Yield` yields only at yieldable points; GC interrupts are ignored
(`src/state.rs:909-949`; the spike already covers the safepoint caveat).

0.12.3 adds `Lua::set_interrupt_oneshot` (`src/state.rs:896`) returning an
`InterruptHandle` (`src/luau/interrupt.rs`). The handle lives behind an `Arc<Mutex<…>>`
and `arm()` writes Luau's interrupt function pointer directly into the callback slot
without taking the VM lock. The rustdoc example is a watchdog on another thread.
The interrupt is initially disarmed, auto-disarms before each invocation, and can be
cancelled with `disarm()`. Removing or replacing the interrupt, or dropping the VM,
invalidates all clones; dropping a handle does **not** cancel a pending request.

**Ran:**

| Probe                                                                                              | Result                                                                                            |
| -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `arm()` from a plain OS thread while `lua.load("while true do end").exec()` runs on another thread | accepted, killed after ~56 ms with `runtime error: killed`; a following `return 1+1` gave `Ok(2)` |
| `arm()` after the kill, then `disarm()`                                                            | following chunk gave `Ok(25)` — one-shot auto-disarms, and explicit disarm is harmless            |
| `AtomicBool` flag read by a continuous `set_interrupt`, flag set from another thread               | killed after ~55 ms; VM reusable (`Ok(42)`)                                                       |
| `arm()` from another thread into `exec_async()` on a 3-worker Tokio runtime after an `await`       | killed after ~54 ms; `eval_async` afterwards gave `Ok(16)`                                        |

So both patterns work under async, and arming does not block on the VM mutex even
though the loop's thread holds it.

**Why prefer one-shot:** a continuous callback with a fixed deadline stays armed after
the deadline passes. **Ran:** with `set_interrupt` checking `start.elapsed() >= 400ms`,
a _later, unrelated_ chunk issued after the kill was itself rejected with
`runtime error: deadline`. A one-shot interrupt does not have this failure mode —
it disarms itself after the one invocation. If we keep a continuous callback, the
host must reset or remove the flag/interrupt before reusing the VM.

**Caveat from the source:** the callback only fires at Luau safepoints (function calls
and loop iterations); a single long host C call can delay it, and GC interrupts are
ignored (`src/state.rs:909-949`, <https://luau.org/sandbox>). Host futures therefore
still need their own `tokio::time::timeout`, as the spike concluded.

### Dropping the future is not a stop button

`AsyncThread<R>` is a future/stream wrapping a `Thread` (`src/thread.rs:210`). Its
`Future::poll` calls `self.thread.0.lua.lock()` on every poll (`src/thread.rs:919`)
and holds the guard only until the poll returns. Its `Drop` (`src/thread.rs:818-850`)
uses **`try_lock()`**, not `lock()`: if another thread currently holds the VM, it skips
cleanup entirely. When it does get the lock, it resumes a yielded wrapper thread with
a "terminate" signal, resets it and returns it to the pool. `call_async` marks the
thread recyclable (`src/function.rs:248`); on Luau, `reset_inner` calls
`lua_resetthread` for an errored thread (`src/thread.rs:613-648`), so a coroutine that
died from an interrupt is reset and reused.

**Ran:**

| Probe                                                                                                                                                              | Result                                                                                                                                  |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| Spawn `slow()` (an async host function awaiting a 10 s sleep), `abort()` the task at 50 ms                                                                         | a `Drop` guard inside the Rust future fired (`true`); a following `eval_async` gave `Ok(18)`                                            |
| Spawn `while true do end` via `exec_async()` with a one-shot interrupt armed at 400 ms, `abort()` the task at 50 ms **without awaiting it**, then call into the VM | the second call **blocked 357 ms** before returning `Ok(20)` — the aborted task's loop was still running and the VM lock was still held |
| Same, then `eval` again after the kill                                                                                                                             | `Ok(22)` — VM reusable once the loop was interrupted                                                                                    |

The mechanism matters: aborting a task only drops its future at a poll boundary. A
poll already executing Lua cannot be preempted by a drop on another thread, and
`AsyncThread::drop`'s `try_lock()` deliberately does not wait for the lock. Cancelling
a stuck call therefore requires arming the interrupt, not dropping the future.

### Reusable, or drop and rebuild?

Both work; the tradeoff is small:

- **Interrupt kill:** reusable immediately (`Ran` above). The errored coroutine is
  reset and recycled by mlua (`src/thread.rs:613-648`, `src/state/raw.rs:762`).
- **OOM:** **`Ran`**, with a 4 MiB limit, table growth ended in
  `memory error: not enough memory`; `used_memory()` was still 2.58 MB immediately
  after; `gc_collect()` brought it to 437 KB and `return 11+11` gave `Ok(22)`.
  Collect or rebuild — do not reuse while the garbage is live (spike Q2).
- **Rebuild:** **`Ran`**, creating a fresh VM with the module source loaded took
  ~330 µs; creating + sandboxed 50 VMs averaged ~131 µs each (Q3).

**Decision:** rebuild the VM after a kill or OOM. It is cheap, gives a clean slate for
globals the extension may have mutated, and avoids having to reset the kill flag or
re-arm the interrupt on a reused VM. Reuse is safe when we deliberately keep a one-shot
interrupt registered and want to preserve host-API setup, but rebuild is simpler to
reason about.

## Q2. The `ReentrantMutex`, second callers, and blocked threads

`Lua` wraps `XRc<ReentrantMutex<RawLua>>` (`src/state.rs:58-62`). Under `send`,
`XRc = Arc` and `ReentrantMutex = parking_lot::ReentrantMutex`, which is re-entrant
per **OS thread** (`src/types/sync.rs:4-12`). `Lua::lock()` is a blocking
`ReentrantMutexGuard`, and it panics if the VM is suspended in GC (`src/state.rs:2542`).

**Ran:**

| Probe                                                                                                                          | Result                                                                                |
| ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| Host callback calls `lua.load("return 41+1").eval()` while the outer call holds the lock, on the same OS thread                | `Ok(42)` — re-entrant lock is allowed                                                 |
| While a loop runs on another OS thread (killed by a one-shot interrupt at 400 ms), a second **sync** call from the main thread | blocked **347 ms**, then `Ok(())`                                                     |
| Same, but the second call is `eval_async().await` polled from another Tokio task                                               | the poll blocked **347 ms**, then `Ok(2)`                                             |
| One call suspended at an async host function (`Notify::notified()`), another call from a different task                        | blocked only **53 µs**, `Ok(3)` — the lock is released while awaiting the host future |

**What this means for Autumn:**

- `exec_async`/`call_async` do **not** make lock acquisition non-blocking. `poll()`
  takes the same mutex; a caller whose VM is busy on another thread blocks its Tokio
  worker. With a multi-threaded runtime this is a stall; on a single-worker runtime
  it can deadlock if the busy task needs the blocked task to make progress.
- Every public `Lua` method (`load(...).exec()`, `create_function`, `set_interrupt`,
  `used_memory`, …) locks synchronously. **The UI thread must never call into a VM.**
  All VM work goes through `bridge::run` to a task; a stuck VM stalls that task/worker,
  not the UI, and the watchdog's `arm()` needs no VM lock.
- Run each extension's calls from a single owner task (an actor per VM) so no two
  tasks contend for the same `ReentrantMutex`.

## Q3. Cost of one VM per extension

**Ran** (release, macOS aarch64, 50 VMs unless noted):

| Measurement                                               | Result                                                  |
| --------------------------------------------------------- | ------------------------------------------------------- |
| `Lua::new()`                                              | ~243 µs each (12.1 ms / 50)                             |
| `Lua::new()` + `sandbox(true)`                            | ~131 µs each (6.6 ms / 50)                              |
| `used_memory()` after `Lua::new()`                        | **486,320 B**                                           |
| `used_memory()` after `sandbox(true)`                     | 486,320 B (sandbox itself allocates nothing measurable) |
| after loading a small module                              | 519,040 B                                               |
| compile-from-source (`Chunk::into_function`, new VM each) | ~803 µs each                                            |
| load bytecode (`ChunkMode::Binary`, new VM each)          | ~236 µs each                                            |
| warm source compile (same VM, 1000×)                      | 111 µs each                                             |
| warm bytecode load (same VM, 1000×)                       | **3.5 µs each** (~31× faster)                           |
| bytecode size for a 149 B module                          | 209 B                                                   |

For Luau, bytecode comes from `Compiler::compile(source)` (`src/chunk.rs:398`) and is
loaded with `lua.load(&bytes).set_mode(ChunkMode::Binary)`; `Function::dump` is
compiled out with the `luau` feature (`src/function.rs:486-505`). The bytecode chunk
executed correctly (`parse` returned 3 words).

**Decision:** ship source in Phase 4's extension package, and compile it once at
install/update time into bytecode pinned to the Luau build we ship. Loading bytecode
is ~31× faster than recompiling on every VM startup, while source stays portable and
reviewable. Bytecode is version-bound to Luau — a runtime upgrade must invalidate and
recompile cached bytecode. Host loading of bytecode bypasses `loadstring`'s bytecode
rejection (`src/luau/mod.rs:113`), so only bytecode we compiled ourselves should be
loaded; treat bytecode from a package as untrusted.

Memory is per VM (`Lua::set_memory_limit`, spike Q2), so one VM per extension gives
per-extension caps for free; the ~486 KB baseline each is the price.

## Q4. Rust helpers safely under `send`

With `send`, `MaybeSend`/`MaybeSync` become `Send`/`Sync`
(`src/types.rs:216-242`), and `Lua::create_userdata` requires
`T: UserData + MaybeSend + MaybeSync + 'static` (`src/state.rs:1704-1708`; the
`IntoLua` impl at `src/conversion.rs:285` has the same bounds). So a userdata wrapper
must be `Send + Sync`.

**Ran:**

- A wrapper around `serde_json::Value` (which is `Send + Sync`) was accepted as
  `UserData` with a `get` method; `meta:get('title')` returned "One Piece" and
  `meta:get('year')` returned 1999.
- A compile-fail probe calling `lua.create_userdata(scraper::Html::parse_document(…))`
  was rejected: "`Html` cannot be shared between threads safely", with
  `Sync` not implemented for `OnceCell`/`Cell` internals of `scraper::node::Element`
  and `tendril`. `Arc<Mutex<Html>>` does not help — the inner type is still `!Send`.
- The workable pattern: the host parses the HTML and returns a plain Lua table.
  **`Ran`:** a `create_function` closure selected `li` elements with `scraper`,
  built a `Table`, and Luau read `find_titles()[1] == "Naruto"` and length 2.

**Decision:** the host parses HTML/JSON and returns plain Lua tables (or `String`,
number, bool). Only small `Send + Sync` wrappers become `UserData`. This keeps the
VM's `send` guarantee intact and avoids leaking Rust lifetimes into the sandbox.

## Q5. `require` and the dangerous builtins

mlua installs a filesystem `require` (`FsRequirer`) in `configure_luau`
(`src/luau/mod.rs:107-111`); `sandbox(true)` leaves it in place (spike Q1).

**Ran:** installing a custom require as a Rust `create_function` over an in-memory
map of bundled files **before** `sandbox(true)`, then sandboxing:

- `require('util').double(21)` returned 42; `require('providers/nyaa').name` returned "Nyaa".
- `require('./evil')` failed with "module not found: ./evil" — no filesystem access.
- Removing `getfenv`, `setfenv` and `loadstring` with `globals().raw_set(name, Nil)`
  before sandboxing left all three `Nil`; calling `loadstring` errored with
  "attempt to call a nil value".
- The ordering trap remains: `raw_set("require", Nil)` **after** `sandbox(true)` left
  `type(require) == "function"`, because reads fall through the proxy to the base
  global. All install/remove calls must precede `sandbox(true)`.

For a flat set of bundled files, a `create_function` lookup is enough. mlua also
offers `Lua::create_require_function` (`src/luau/mod.rs:43`) with the full
`Require` trait (`src/luau/require.rs:61-130`), which handles Luau require-by-string
navigation, aliases and config files. That is more machinery than a bundled-file map
needs; revisit it only if extensions adopt aliased/nested module layouts.

**Decision:** before sandboxing, replace `require` with a host function over the
extension's bundled modules and nil out `getfenv`, `setfenv` and `loadstring`.
`require` is thus an explicit host API call, not a filesystem primitive.

## Open question

- Source vs bytecode and Luau upgrades: if we cache install-time bytecode, what
  invalidates the cache when the Luau version changes? The manifest/install ticket
  should decide whether the package stores source, bytecode, or both. Not raised as
  an issue here.

## What was run

The throwaway crate lives in the session scratchpad (not committed). It depends on:

```toml
mlua = { version = "0.12", features = ["luau", "async", "send"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
scraper = "0.23"
serde_json = "1"
```

This resolved to mlua 0.12.3, mlua-sys 0.14.0, luau0-src 0.24.1+luau748. Binaries,
all built with `cargo build --release` on rustc 1.100.0, macOS aarch64:

1. **`cancel`** — one-shot arm from a thread, `AtomicBool` + continuous interrupt,
   arm into `exec_async` on Tokio, dropping a future suspended in a host future,
   aborting mid-loop, and VM reuse after each.
2. **`reentrancy`** — reentrant host callback, second sync/async call into a looping
   VM, and a call into a VM suspended at a host future.
3. **`vms`** — VM creation time, baseline memory, source vs bytecode, and OOM
   recovery vs rebuild.
4. **`userdata_ok`** — `serde_json::Value` as `UserData` and host-parsed HTML as a
   plain table.
5. **`userdata_fail`** — compile-fail probe for `scraper::Html` as `UserData`, run
   with `cargo check --bin userdata_fail --features fail-probe`.
6. **`require_probe`** — bundled-file require, builtin removal, and the
   after-sandbox ordering trap.

The spike's 0.11.6/0.12.2 results and this 0.12.3 run agree on the underlying
mechanics; the material difference is the new `set_interrupt_oneshot` API.
