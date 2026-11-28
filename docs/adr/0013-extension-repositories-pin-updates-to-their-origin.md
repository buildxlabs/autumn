# Extension repositories are HTTPS JSON indexes with origin-pinned, reviewed updates

Autumn discovers extensions from user-added HTTPS JSON repositories (`schema_version: 1`, 2 MiB cap) and from direct HTTPS manifest URLs; local file installs are not allowed in Phase 4, and local authoring happens in the extension playground. Trust rests on HTTPS, an optional `sha256` payload digest (no author signatures yet), host and settings disclosures before install, and `origin_url` pinning: update checks query only the installing origin, and installing the same `id` from another repository is an explicit replacement. Updates never install on their own: each one is reviewed (metadata, declared-host changes, settings-schema impact, payload diff), can be skipped by version, and keeps a one-slot local rollback. Autumn ships with zero pre-seeded repository URLs; first-party extensions live in a separate `buildxlabs/autumn-extensions` repository that the user adds by URL, which keeps `buildxlabs/autumn` free of bundled scrapers (D8). Decided on #266, with the first-party repository policy from #274.

## Consequences

- A popular `id` advertised by several repositories cannot silently hijack updates, and host-permission widening (ADR 0015) and settings migrations (ADR 0012) are visible before new code runs.
- Repositories that serve only the latest release are still safe to update from, because the user can roll back one version locally.
- A failing or removed repository never disables installed extensions: a failed refresh keeps the last cached listings, and removing a repository leaves its extensions installed and runnable, with updates paused.

## Considered options

- **Multi-format community indexes, or URL-only lists.** Extra parser surface under D10, or N+1 round trips just to browse.
- **Mandatory author signatures, or `http://` and local file installs.** Premature key distribution given ADR 0014 and ADR 0015; local iteration belongs in the playground.
- **Silent auto-updates, cross-repository auto-updates, or no rollback.** They break diff-before-install and leave users stuck on a broken release.
- **A preset first-party repository URL in the binary.** It bakes a mutable external URL into releases and works against D8.
