# Extensions run in Luau, against our own API

Extensions run in Luau through `mlua`, with declarative sources (a manifest of URL templates and selectors, no code) for simple indexers and feeds. A WASM runtime (wasmtime or Extism) may come later for heavy or compiled extensions, behind the same provider API once that API is stable. There is no JavaScript runtime, although the original research planned on QuickJS: we would rather not maintain a JS sandbox, and Luau ships a built-in sandbox with memory limits and interrupts. Extensions target Autumn's own manifest and host API, not an existing community format, and the community extensions we need are ported by hand.

## Consequences

- The spike in [`docs/research/mlua-luau-sandbox.md`](../research/mlua-luau-sandbox.md) confirmed the sandbox holds, on two conditions. Each extension gets its own `Lua` VM. mlua's default filesystem `require` survives `sandbox(true)`, so it is removed or replaced with our own before sandboxing.
- Memory limits and interrupts cover code running inside the VM. Time spent awaiting a host future needs its own Tokio timeout.
- Existing JS-based extension ecosystems can't be loaded directly. Each one is a hand port.

## Considered options

- **JavaScript via `rquickjs`.** It matches the research and most community extensions, but it means maintaining a JS barrier and its sandbox.
- **WASM first.** Strong isolation (fuel or epoch interrupts, resource limiters), but it is heavy for small scrapers. It stays the later option.
- **Compatibility with an existing extension format.** It would tie our host API to someone else's design and its churn.
