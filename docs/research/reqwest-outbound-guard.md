# Enforcing the extension outbound guard in reqwest

Research for [#261](https://github.com/buildxlabs/autumn/issues/261). It settles
the "outbound guard" part of D8 and ARCHITECTURE §6 (Extensions): extension
network access goes through `net` with no private, loopback or link-local
addresses. The shared client lives in `crates/net` (`Client::build`,
`build_reqwest` in `crates/net/src/client.rs:433-452`); the proxy setting comes
from [#210](https://github.com/buildxlabs/autumn/issues/210) (ADR-0010) and is
already implemented as `ClientBuilder::proxy` / `Client::set_proxy`.

Researched 2026-11-28 against **reqwest 0.13.5** (root `Cargo.toml` pins `0.13`,
`Cargo.lock` resolves 0.13.5), hyper 1.12.0, hyper-util 0.1.21, url 2.5.9 and
ipnet 2.12.2, on rustc 1.100.0 / macOS aarch64.

- `src/…` paths point into the reqwest 0.13.5 crate source
  ([docs.rs source](https://docs.rs/reqwest/0.13.5/source/)), unless the path
  says `hyper-util`, which points into hyper-util 0.1.21.
- "Ran" marks results from the throwaway probe described in
  [What was run](#what-was-run). That crate was not committed.

## Verdict

**Yes — but it takes three separate hooks, and a proxy weakens it.**

1. **A custom `dns_resolver` is the primary guard.** Wrap the default system
   resolver and reject (or filter) private addresses before returning them.
   reqwest/hyper resolve each name once per connection and connect to exactly
   those addresses, so a resolve-then-connect rebind race is closed.
2. **IP literals bypass the resolver**, including `localhost`, `[::1]`,
   `0.0.0.0`, IPv4-mapped IPv6, and integer/octal/hex spellings that `url`
   normalizes to an IPv4 literal. A guard must also check the URL in `net`
   before sending, and use a custom `redirect::Policy` for each hop.
3. **A proxy changes what the client can see.** With an HTTP proxy (or
   `socks5h`), the target name is resolved by the proxy, so no client-side IP
   check is possible. With the `socks5://` scheme Autumn currently accepts,
   reqwest uses _our_ resolver locally, so the guard still holds. Also call
   `.no_proxy()` when no proxy is configured: otherwise environment proxies
   can silently divert traffic around the resolver.

`Response::remote_addr()` exists in 0.13.5, but it is **post-connect**: useful
as defence in depth and for diagnostics, not as the guard. With a proxy it
reports the proxy, not the target.

## Q1. Which reqwest 0.13.5 hook does what?

| Hook                                           | Sees                                  | Initial IP literal | Redirect to literal | Redirect to name       | Closes rebinding | With HTTP proxy      |
| ---------------------------------------------- | ------------------------------------- | ------------------ | ------------------- | ---------------------- | ---------------- | -------------------- |
| `dns_resolver` (`ClientBuilder::dns_resolver`) | name → addresses, once per connect    | no                 | no                  | yes                    | **yes**          | sees proxy host only |
| URL check in `net` before `execute`            | the request URL                       | **yes**            | only initial URL    | no (hops are internal) | no               | **yes**              |
| `redirect::Policy::custom`                     | each `Location` URL before following  | no                 | **yes**             | yes                    | no               | **yes**              |
| `connector_layer`                              | opaque wrapper around the `Uri`       | no                 | no                  | no                     | no               | sees proxy `Uri`     |
| `Response::remote_addr()`                      | connected TCP peer, after the request | no                 | no                  | no                     | no               | reports the proxy    |

### `dns_resolver` — use this for names

`ClientBuilder::dns_resolver` installs an `Arc<dyn Resolve>`
(`src/async_impl/client.rs:2327-2333`). The trait is:
`fn resolve(&self, name: Name) -> Resolving`, where `Resolving` is a boxed
future of `Result<Addrs, BoxError>` and `Addrs` is a boxed iterator of
`SocketAddr` (`src/dns/resolve.rs:12-31`; `Name::as_str()` at `:42-45`).
Resolution errors are tagged so `Error::is_dns()` recognises them
(`src/dns/resolve.rs:110-115`).

At build time, reqwest uses hyper-util's `GaiResolver` (system `getaddrinfo`
through Tokio) unless a custom resolver is supplied
(`src/async_impl/client.rs:428-445`; `src/dns/gai.rs:1-25`). **Overrides from
`resolve_to_addrs` are applied _after_ the custom resolver**
(`src/async_impl/client.rs:434-440`, `src/dns/resolve.rs:97-117`), so do not use
those builder methods on the guarded client — they can inject addresses that
never pass the guard. This workspace does not enable reqwest's `hickory-dns`
feature, so the fallback is the system resolver, not DoH; ARCHITECTURE's
DNS-over-HTTPS item is still separate.

**Why rebinding is closed.** hyper-util's connector tries
`dns::SocketAddrs::try_parse(host, port)` first and only calls the resolver for
a real name; the addresses returned by that one call are collected and handed
to `ConnectingTcp`, which connects to them (with fallback between them), with
no second lookup (`hyper-util` `src/client/legacy/connect/http.rs:553-572`).
Check every address in the iterator before returning it. Returning an empty
iterator makes the connect fail; returning an error is clearer and shows up as
a DNS error.

### `redirect::Policy::custom` — needed for hops

`Policy::custom` gets an `Attempt` with `status()`, `url()` (the next URL) and
`previous()` (`src/redirect.rs:58-100`, `:168-182`). The resolver _is_ called
again for a redirect target that is a name — **ran**: a redirect to
`private.test` showed `["public.test", "private.test"]` in the resolver log.
A redirect to an IP literal is not — **ran**: a redirect to `127.0.0.1` showed
only `["public.test"]`. So the policy must run the URL check on `attempt.url()`.

Two details:

- The custom variant **does not inherit the default 10-hop limit**; the docs
  say so explicitly and `Policy::default()` is `limited(10)`
  (`src/redirect.rs:66-70`, `:127-150`). Keep a hop counter in the closure.
- `Policy::redirect()` can wrap another policy (`src/redirect.rs:102-125`),
  which lets us reuse `Policy::limited(10)` after our own check.

### `connector_layer` — not a post-connect hook

reqwest 0.13.5 has no public `ClientBuilder::connector()` override. It has
`connector_layer` (`src/async_impl/client.rs:2475-2492`), a `tower::Layer`
around the connector service. The layer's service receives
`sealed::Unnameable(Uri)`, a private wrapper (`src/connect.rs:1335-1338`), so a
custom layer sees the request `Uri` _before_ resolution, not the peer address
after it. Its documented uses are timeouts and concurrency limits. It cannot
implement the guard.

### `remote_addr()` — detection only

`Response::remote_addr()` returns `HttpInfo::remote_addr()`
(`src/async_impl/response.rs:114-121`); hyper-util sets that extension from
`TcpStream::peer_addr()` when the connection is made
(`hyper-util` `src/client/legacy/connect/http.rs:588-595`). It is available in
0.13.5. By the time it is readable the request has already been sent, so it
can stop the body reaching an extension but cannot undo a side effect.
**Ran:** without a proxy it reported the target; through an HTTP proxy it
reported the proxy's address (`127.0.0.1:<proxy port>`), not the target's.

Existing middleware is not needed to get a pre-send URL check: `net` already
funnels every request through `Client::send` (`crates/net/src/client.rs:293`),
so the check belongs there and in `Request::into_reqwest`
(`crates/net/src/request.rs:95`). A `reqwest-middleware`-based automatic check
is an alternative (see [Q4](#q4-who-else-does-this-and-under-which-license)).

## Q2. What exactly must be blocked?

### Literals and name spellings

- `url` 2.5.9 normalises decimal, octal and hex IPv4 spellings to a real IPv4
  host. **Ran:** `Url::parse("http://2130706433/")` and
  `Url::parse("http://0177.0.0.1/")` both have `host_str() == "127.0.0.1"`.
  Those requests skip the resolver entirely (**ran**), so the URL check must
  look at the parsed `Url::host()`, not a regex on the string.
- `[::ffff:127.0.0.1]` is `Host::Ipv6` in `url`, parses as an `Ipv6Addr` in
  hyper-util's `try_parse`, and therefore skips the resolver (**ran**; it
  connected to a loopback listener and the final URL normalised to
  `[::ffff:7f00:1]`). `Ipv6Addr::to_ipv4()` and `to_ipv4_mapped()` are stable
  in 1.100; **ran:** `to_ipv4()` returns `Some(127.0.0.1)` for both
  `::ffff:127.0.0.1` (mapped) and `::127.0.0.1` (compatible), while
  `to_ipv4_mapped()` only handles the mapped form. Unwrap to IPv4 and apply the
  v4 rules.
- NAT64 `64:ff9b::/96` (RFC 6052), local-use NAT64 `64:ff9b:1::/48` (RFC 8215)
  and 6to4 `2002::/16` (RFC 3056) embed IPv4 addresses; unpack them and apply
  the v4 rules. Blanket-denying `64:ff9b::/96` would break IPv6-only networks
  where public indexers are reached through NAT64.
- `0.0.0.0` is not inert: **ran**, `http://0.0.0.0:<port>/` connected to a
  loopback listener on macOS.
- Names that must be denied even when a resolver would catch the address:
  `localhost` and `*.localhost` (RFC 6761 §6.3 says resolvers SHOULD return
  loopback for them), and `.local` (RFC 6762 §3: link-local names whose
  queries go to `224.0.0.251`/`FF02::FB`). Normalise with lowercase and a
  trailing-dot strip before suffix matching. This is belt-and-braces: public
  domains such as `localtest.me` or wildcard DNS services also resolve to
  loopback, and only the IP check catches those.

### Full special-purpose range list

Source: [IANA IPv4 registry](https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry.xhtml)
and [IANA IPv6 registry](https://www.iana.org/assignments/iana-ipv6-special-registry/iana-ipv6-special-registry.xhtml),
both last updated 2025-10-09. "GR" is the registry's _Globally Reachable_
column. Deny every entry with GR `False`, plus multicast (RFC 1112 §4 and
RFC 4291 §2.7, not in the special-purpose tables). The GR `True` entries are
protocol anycast, documentation and benchmarking blocks; an anime extension
has no reason to contact them, so denying the whole table is a reasonable
conservative choice — except for the translation prefixes, which should be
unpacked as described above.

IPv4:

| Block                              | Name                            | GR    |
| ---------------------------------- | ------------------------------- | ----- |
| `0.0.0.0/8` (`0.0.0.0/32`)         | This network / this host        | False |
| `10.0.0.0/8`                       | Private-Use (RFC 1918)          | False |
| `100.64.0.0/10`                    | Shared / CGNAT (RFC 6598)       | False |
| `127.0.0.0/8`                      | Loopback                        | False |
| `169.254.0.0/16`                   | Link Local (RFC 3927)           | False |
| `172.16.0.0/12`                    | Private-Use                     | False |
| `192.0.0.0/24`                     | IETF Protocol Assignments       | False |
| `192.0.0.0/29`                     | IPv4 Service Continuity         | False |
| `192.0.0.8/32`                     | IPv4 dummy address              | False |
| `192.0.0.9/32`, `192.0.0.10/32`    | PCP / TURN anycast              | True  |
| `192.0.0.170/32`, `192.0.0.171/32` | NAT64/DNS64 discovery           | False |
| `192.0.2.0/24`                     | Documentation TEST-NET-1        | False |
| `192.31.196.0/24`                  | AS112-v4                        | True  |
| `192.52.193.0/24`                  | AMT                             | True  |
| `192.88.99.0/24`                   | Deprecated (6to4 relay anycast) | —     |
| `192.88.99.2/32`                   | 6to4 relay anycast              | False |
| `192.168.0.0/16`                   | Private-Use                     | False |
| `192.175.48.0/24`                  | Direct Delegation AS112         | True  |
| `198.18.0.0/15`                    | Benchmarking (RFC 2544)         | False |
| `198.51.100.0/24`                  | Documentation TEST-NET-2        | False |
| `203.0.113.0/24`                   | Documentation TEST-NET-3        | False |
| `224.0.0.0/4`                      | Multicast (RFC 1112, RFC 5771)  | False |
| `240.0.0.0/4`                      | Reserved (RFC 1112 §4)          | False |
| `255.255.255.255/32`               | Limited Broadcast               | False |

IPv6:

| Block                                             | Name                             | GR           |
| ------------------------------------------------- | -------------------------------- | ------------ |
| `::1/128`                                         | Loopback                         | False        |
| `::/128`                                          | Unspecified                      | False        |
| `::ffff:0:0/96`                                   | IPv4-mapped — unwrap and recheck | False        |
| `64:ff9b::/96`, `64:ff9b:1::/48`                  | NAT64 translation — unwrap       | True / False |
| `100::/64`                                        | Discard-Only                     | False        |
| `100:0:0:1::/64`                                  | Dummy prefix (RFC 9780)          | False        |
| `2001::/23`                                       | IETF Protocol Assignments        | False        |
| `2001::/32`                                       | Teredo                           | N/A          |
| `2001:1::1/128`, `2001:1::2/128`, `2001:1::3/128` | PCP / TURN / DNS-SD anycast      | True         |
| `2001:2::/48`                                     | Benchmarking                     | False        |
| `2001:3::/32`                                     | AMT                              | True         |
| `2001:4:112::/48`                                 | AS112-v6                         | True         |
| `2001:10::/28`                                    | Deprecated (ORCHID)              | —            |
| `2001:20::/28`, `2001:30::/28`                    | ORCHIDv2, Drone Remote ID        | True         |
| `2001:db8::/32`, `3fff::/20`                      | Documentation                    | False        |
| `2002::/16`                                       | 6to4 — unwrap and recheck        | N/A          |
| `2620:4f:8000::/48`                               | Direct Delegation AS112          | True         |
| `5f00::/16`                                       | Segment Routing SIDs             | False        |
| `fc00::/7`                                        | Unique-Local                     | False        |
| `fe80::/10`                                       | Link-Local Unicast               | False        |
| `ff00::/8`                                        | Multicast (RFC 4291 §2.7)        | False        |

**Implementation note.** `IpAddr::is_global()` is still unstable on rustc
1.100 (issue #27709) — **ran**. Stable helpers cover only part of the table:
`Ipv4Addr::is_private()` does **not** include `100.64/10`, `0/8`,
`192.0.0.0/24` or `240/4`. Use explicit CIDRs; `ipnet` 2.12.2 (MIT OR
Apache-2.0) is already in `Cargo.lock`, so adding it to `net` costs no new
transitive crate.

## Q3. What does a configured proxy do to the guard?

`build_reqwest` calls `reqwest::Proxy::all(url)` for an `http://` or
`socks5://` URL (`crates/net/src/client.rs:433-452`, `validate_proxy` at
`:456-474`); `set_proxy` rebuilds the client on a settings change (`:234-249`).

- **Environment proxies are a silent hole.** reqwest's `ClientBuilder::proxy`
  docs note that adding a proxy disables automatic system proxies, and
  `no_proxy()` clears proxies _and_ disables system proxy use
  (`src/async_impl/client.rs:1423-1440`). `build_reqwest` never calls
  `no_proxy()`, so with no configured proxy, `HTTP_PROXY`/`HTTPS_PROXY`/
  `ALL_PROXY` can be honoured and the resolver will see only the proxy host.
  Call `.no_proxy()` for the unproxied client.
- **HTTP proxy:** the connector connects to the proxy URI and sends the target
  in absolute form; for HTTPS it builds a `Tunnel` to the proxy and sends the
  target host in CONNECT (`src/connect.rs:783-895`; hyper-util
  `src/client/legacy/connect/proxy/tunnel.rs:58-67`, `:133-151`). The target is
  never resolved locally. **Ran:** with `http://proxy.test:<port>`, only
  `proxy.test` appeared in the resolver log and `remote_addr()` was the proxy.
  The guard can still enforce URL-level rules (literal IPs, `.local`, port
  checks) and the proxy's own egress policy; it cannot enforce the target's IP.
- **SOCKS5:** `connect_socks` sets `DnsResolve::Local` for `socks4`/`socks5`
  and `DnsResolve::Proxy` for `socks4a`/`socks5h`
  (`src/connect.rs:536-544`). For `Local`, `socks::connect` calls
  `resolver.http_resolve(&dst)` (`src/connect.rs:1918-1925`), and the builder
  passes our resolver into the connector via `set_socks_resolver`
  (`src/async_impl/client.rs:931`, `src/dns/resolve.rs:63-88`). **Ran:** the
  resolver saw `public.test` with `socks5://` and did not with `socks5h://`
  (the probe's minimal SOCKS server did not complete the handshake, so only
  the resolver observation is used here; the source is the authority). Because
  `validate_proxy` currently rejects `socks5h`, Autumn's supported SOCKS5
  proxy keeps the guard intact.
- **HTTPS-only hardening:** `https_only(true)` makes internal HTTP-only
  services (cloud metadata) unreachable and forces a valid certificate for
  the requested name, but it also opens a TCP connection first and breaks
  plain-HTTP indexers. Consider it only for extension traffic, not the shared
  client as a whole.

**Consequence for the decision:** a hard "no private addresses" promise is
only true for the unproxied and `socks5://` cases. With an HTTP proxy the
client cannot see the destination IP; either the proxy must enforce egress, or
the guarantee must be explicitly relaxed and surfaced to the user. That is an
open question for the host-API/settings work, not something this research
decides.

## Q4. Who else does this, and under which license?

| Project                                                                                  | Approach                                                                                                                                                                                                                                                                                            | License                                                                                                                                                     | Relevance                                                                                                                                  |
| ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| [reqwest-ssrf-guard 0.2.0](https://docs.rs/reqwest-ssrf-guard/0.2.0/reqwest_ssrf_guard/) | `Acl` installs a resolver + redirect policy; optional `middleware` feature calls `validate_url` on every request to cover the initial literal. `deny_local_network` includes CGNAT, IPv4-mapped, NAT64 and 6to4; host-suffix and port rules. Its docs call out `no_proxy()` and `https_only(true)`. | MIT ([Cargo.toml](https://github.com/walf443/reqwest-ssrf-guard/blob/main/Cargo.toml))                                                                      | Closest match, reqwest 0.13; MSRV 1.87. Its deny set is not the full IANA table (its tests allow `192.0.2.0/24`), so extend it if adopted. |
| [url_jail 0.2.0](https://docs.rs/url_jail/0.2.0/url_jail/)                               | Validate URL **and** resolved IP, then connect with `resolve()` to the validated address so no second lookup can rebind; `fetch()` validates each redirect; all returned IPs checked.                                                                                                               | MIT OR Apache-2.0                                                                                                                                           | Good source for the validate-then-pin pattern and the list of encoded IP tricks.                                                           |
| [Extism manifest](https://extism.org/docs/concepts/manifest/)                            | `allowed_hosts` is a declarative host list for plugin HTTP: empty = no hosts, `null` = all. No IP-level check.                                                                                                                                                                                      | BSD-3-Clause ([workspace Cargo.toml](https://github.com/extism/extism/blob/main/Cargo.toml), [LICENSE](https://github.com/extism/extism/blob/main/LICENSE)) | Pattern for a per-extension host allowlist in a manifest; not a substitute for the IP guard.                                               |
| [`ipnet` 2.12.2](https://docs.rs/ipnet/2.12.2/ipnet/)                                    | `IpNet::contains` for CIDR checks; stable only.                                                                                                                                                                                                                                                     | MIT OR Apache-2.0                                                                                                                                           | Already a transitive dependency; put the IANA table in a static array.                                                                     |

`deny.toml` already allows MIT, Apache-2.0 and BSD-3-Clause, so none of these
need a policy change. `reqwest-middleware` 0.5.2 is MIT OR Apache-2.0 if the
automatic URL validation is wanted.

## Open questions surfaced

- If a global HTTP proxy is configured, what is the promised behaviour?
  Options: require proxy-side egress filtering, warn in Settings that the
  guard is reduced, or restrict guarded extension traffic to no proxy /
  `socks5://`. This needs a product decision with the Network settings ticket.
- NAT64: unpack-and-check is recommended over blanket-denying `64:ff9b::/96`,
  but that assumes we want extensions to work on IPv6-only networks.
- Should the extension manifest carry an Extism-style `allowed_hosts` list?
  It would help with `.local`/name rules but needs a host-API decision.

## What was run

A throwaway crate in the session scratchpad (not committed) with
`reqwest = "=0.13.5"` (`default-features = false`, `socks`) and Tokio. It ran a
local HTTP server and installed a `Resolve` impl mapping every name to
`127.0.0.1:0` (reqwest substitutes the URL port), logging every `Name` passed
to the resolver and each response's `remote_addr()`.

| Probe                                | Resolver saw                  | Result                                   |
| ------------------------------------ | ----------------------------- | ---------------------------------------- |
| `http://public.test:<port>/a`        | `public.test`                 | connected, `remote_addr` = target        |
| `http://localhost:<port>/b`          | `localhost`                   | connected                                |
| `http://127.0.0.1:<port>/c`          | —                             | connected                                |
| `http://[::1]:<port>/d`              | —                             | connection refused (no v6 listener)      |
| `http://[::ffff:127.0.0.1]:<port>/e` | —                             | connected; URL became `[::ffff:7f00:1]`  |
| `http://2130706433:<port>/f`         | —                             | connected; URL became `127.0.0.1`        |
| `http://0.0.0.0:<port>/g`            | —                             | connected to loopback listener           |
| redirect to `private.test`           | `public.test`, `private.test` | policy saw each hop, both resolved       |
| redirect to `127.0.0.1`              | `public.test` only            | resolver never saw the literal           |
| `proxy = http://proxy.test:<p>`      | `proxy.test` only             | `remote_addr` = proxy, not target        |
| `proxy = socks5://127.0.0.1:<p>`     | `public.test`                 | local DNS; SOCKS handshake not completed |
| `proxy = socks5h://127.0.0.1:<p>`    | —                             | remote DNS; handshake not completed      |

Not tested: TLS and certificate behaviour, Linux and Windows loopback
semantics, and a fully working SOCKS5 endpoint. Those conclusions rest on the
reqwest/hyper-util source and the RFCs cited above.
