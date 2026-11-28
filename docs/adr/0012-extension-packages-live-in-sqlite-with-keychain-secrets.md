# Extension packages live atomically in SQLite, with secret settings in the OS keychain only

An installed extension is a validated JSON manifest and one inlined payload (64 KiB descriptor, 512 KiB payload) stored as a row in `autumn.db`, identified by a globally unique slug `id` and the `origin_url` it was installed from. Non-secret settings sit in SQLite keyed by `extension_id`; `secret` setting values live only in the OS keychain (`keyring`, account `ext:<id>:<key>`), under the same rule as ADR 0002. A SemVer `version` orders updates, and an integer `api_version` (`1` in Phase 4) decouples extensions from app releases. Decided on #265.

## Consequences

- Install, update, `(key, type)` settings migration and uninstall each run in one SQLite transaction, with no orphaned files under `<data>/extensions/`.
- Private-tracker API keys, passkeys and cookies never touch SQLite, logs or script source. On a machine without a usable keychain, saving a `secret` field is refused and the extension stays in `needs_configuration`; extensions without secrets still run.
- Disabling an extension stops its VM and keeps its settings and keychain entries; uninstalling deletes both the rows and the `ext:<id>:*` keychain entries.

## Considered options

- **Loose `<data>/extensions/<id>.json` files or archive bundles.** Torn state between files and SQLite, and an archive/path-traversal surface.
- **Plaintext SQLite or a passphrase-encrypted file for `secret` fields.** Rejected for the same reasons as ADR 0002.
- **seanime's `userConfig.version` gate and `{{field}}` code interpolation.** A manual config version bump wipes unchanged settings, and interpolating into code risks injection and puts secrets in source text.
- **An app SemVer range as the compatibility check.** An app release would break extensions even when the host API did not change.
