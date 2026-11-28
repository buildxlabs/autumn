# Extensions run under fixed caps, one call at a time per VM, and a killed VM is rebuilt

A looping or allocating Luau extension holds its VM's synchronous `ReentrantMutex`, stalling any second caller on that VM until an interrupt fires, and dropping a future cannot preempt Luau that is already running (#262). Each Luau extension therefore gets its own `Lua` VM behind a single-caller Tokio actor with fixed Phase 4 limits: 32 MiB memory, 1 s of active Luau CPU time enforced with `set_interrupt_oneshot`, a 30 s wall-clock timeout that includes host futures, and 8 HTTP requests per call. The VM is dropped and rebuilt after any kill, OOM or uncaught error, and an extension is auto-disabled after 3 consecutive CPU or memory kills. A kill re-arms the interrupt until the call unwinds, so `pcall` cannot swallow it. Decided on #269, with uncatchable kills from #267.

## Consequences

- The GPUI main thread never touches a VM lock (ARCHITECTURE §4), and one looping or OOMing extension cannot freeze the UI or delay other extensions.
- Caller cancellation or the 30 s timeout drops suspended host futures (cancelling in-flight `net` requests) at once and arms the interrupt if Luau is running.
- A `MemoryError` caught by `pcall` is still reported as a memory kill, and a call's outcome is the kill whatever the script returns.
- Transient network, wall-timeout and parse errors show in extension health and the Extensions sidebar badge, without toasts and without auto-disabling; only the third consecutive CPU or memory kill auto-disables and raises a one-shot error toast.
- The 9th HTTP request in a call is an uncatchable kill like the CPU and memory caps: the VM is rebuilt and the kill's message names `request_limit`. It does not count toward auto-disable.
- `auto_disabled` and `last_error` (kind, message, time) persist on the extension's row in `autumn.db`; the traceback and the strike counter stay in memory.
- A search-deadline cancel (ADR 0011) is not a failure: it is not recorded in health and shows only in the release list's partial-failure notice.
- Declarative sources run no user code, so the CPU and memory caps and the auto-disable rule do not apply to them (ADR 0016).

## Considered options

- **A raw instruction or tick count, or user- or manifest-tunable limits.** Safepoint cost varies widely by operation, and overrides add settings and let a buggy manifest raise its own caps.
- **Concurrent calls in one VM, or reusing a VM after a kill.** Interleaved coroutines race on shared globals across `await` and die together on a kill, while a rebuild costs under 1 ms (#262).
- **Auto-disable on the first failure, or never.** A network blip would disable a working extension, or a broken loop would burn CPU on every search.
