# Network settings fail closed on a bound interface, split proxies by traffic class, and only rate limits apply live

librqbit 9.1 exposes `bind_device_name` (macOS `IP_BOUND_IF`, Linux `SO_BINDTODEVICE`, no Windows equivalent), a SOCKS5-only `connect.proxy_url`, construction-time listen/UPnP/DHT options and a mutable session `ratelimits`; `net` is one shared HTTP client with no proxy wired up today. Neither indexer publishes a numeric rate limit or maintains a mirror list, and every torrent stream gets its own relay token (ADR-0005). We fail closed when a bound interface is unavailable, split one SOCKS5 proxy for torrent peers from one global proxy for app HTTP, make both indexer base URLs configurable without failover, and apply only rate limits live.

## Consequences

- A VPN interface rename needs a module restart, and peers bypass interface binding when the SOCKS5 proxy is set.
- A dead mirror surfaces `IndexerError` instead of silently failing over; global limits throttle preloads too, so per-stream bandwidth policy stays outside these settings.
- Other torrent changes restart the module after playback ends, so a stream is never interrupted by a settings save.

## Considered options

- **Fallback with a warning, one proxy for all traffic, per-indexer proxies, a mirror list with failover, immediate session rebuild and a Test button** — all rejected: they leak traffic, need extra clients, invent mirrors that do not exist, or kill a live stream.
