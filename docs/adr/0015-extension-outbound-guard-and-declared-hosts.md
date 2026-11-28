# Extensions reach only declared public hosts behind net's outbound guard

Extension HTTP goes through `net` with three enforcement hooks (#261): a pre-send URL check for IP literals, a validating `dns_resolver` that rejects any resolution containing a blocked address (closing DNS rebinding), and a `redirect::Policy::custom` with a 10-hop cap that re-checks every `Location`; with no proxy configured the client calls `.no_proxy()`, so environment proxies cannot bypass the resolver. The guard allows only `http` and `https`, and blocks `localhost`, `*.localhost` and `*.local`, multicast, reserved and broadcast ranges, and every IANA `Globally Reachable = False` IPv4 and IPv6 block, after unpacking IPv4-mapped, IPv4-compatible, NAT64 and 6to4 addresses. Every manifest declares the hosts it may contact, and a new or widened host on update is a permission change; only an explicit per-extension user setting may allow a specific LAN host. Decided on #270.

## Consequences

- A manifest can never grant private-network access, and an extension that moves to an undeclared mirror or CDN fails closed until it is updated or the user adds an override.
- Extension traffic follows ADR 0010's global `http_proxy`. Under `socks5://` the full IP guard holds; under `http://` the proxy resolves targets, so `net` still enforces scheme, literals, declared hosts and redirects, and Network settings warns that IP-level enforcement shifts to the proxy.
- Requests are stateless (no shared cookie jar), hop-by-hop headers are stripped, responses are capped at 4 MiB and requests at 1 MiB, and each extension has its own 30 req/min rate limit with no hidden retries.
- A blocked call fails at once in Luau, is recorded on that extension's health without toasts, and shows inline in the playground.

## Considered options

- **An RFC 1918 and loopback-only blocklist, a blanket NAT64 deny, or no manifest host list.** Leaves CGNAT and mapped-IPv6 bypasses open, breaks IPv6-only networks, or removes install-time and update-time permission visibility.
- **No LAN override, or bypassing or blocking `http://` proxies.** Locks out self-hosted indexers, leaks traffic around ADR 0010's proxy, or breaks users behind HTTP proxies.
- **A shared cookie jar and automatic `net` retries.** Risks credential leakage across extensions and app sessions, and eats the caller's retry and timeout budget.
