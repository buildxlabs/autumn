# Extension packaging, distribution, install and update in seanime and mangayomi

Research for [#259](https://github.com/buildxlabs/autumn/issues/259). Researched
2026-11-28 against the read-only references at
`/Users/gaurav/personal/projects/autumn/reference/`. Citations use paths relative
to that directory, with line numbers.

Sources and licenses:

- `reference/seanime/internal/{extension,extension_repo,extension_playground}` —
  **GPL-3.0** (repo-root `LICENSE`, GNU GPL v3). Behavior is the target; ported
  code, if any, keeps the original notice.
- `reference/mangayomi` — **Apache-2.0** (repo-root `LICENSE`, Apache 2.0).
  Confirmed per file: no separate license headers were found in the Dart files
  read below (`extension_store_service.dart`, `extension_update.dart`,
  `fetch_sources_list.dart`, `source_repositories.dart`, `models/source.dart`,
  `services/source_code_errors.dart`, `utils/error_toast.dart`), so the repo-wide
  license governs them. No AGPL reference was used.

## Verdict

Both apps treat an extension as a **small descriptor plus a code blob**, fetched
over plain HTTP(S) and persisted locally. Neither verifies a hash or signature
of the downloaded code: trust rests on the descriptor schema, a version check,
and (in seanime) an app-version constraint. Both key identity to where an
extension came from, so a same-ID extension from another repository is a
different thing. Updates replace the stored code then reload/re-run it; removal
clears the code and its settings. Errors are surfaced as structured "invalid"
records or as toasts/banners, not by failing the whole app.

For Autumn, the decision-relevant pieces are: a manifest schema with explicit
required fields; an origin URL carried with every install; a version comparison
that gates updates; user-config versioning; and an explicit unload/interrupt
step before replacing code. Details follow.

## seanime (GPL-3.0)

### Package format and enforced manifest fields

`extension.Extension` is the manifest struct: `id`, `name`, `version`,
`semverConstraint`, `manifestURI`, `language`, `type`, `description`, `author`,
`icon`, `website`, `readme`, `notes`, `lang`, `permissions`, `userConfig`,
`payload`, `payloadURI`, `plugin`, `isDevelopment` (`extension/extension.go:27-75`).
`manifestURI` is `"builtin"` for built-in extensions and `""` for local ones;
otherwise it is the URL the manifest came from (`extension/extension.go:35-37`,
`extension_repo/builtin.go:11-15`).

`manifestSanityCheck` enforces (`extension_repo/utils.go:36-90`):

- required non-empty: `id`, `name`, `version`, `language`, `type`, `author`;
  plus at least one of `payload` / `payloadURI`;
- `id` is 3–40 chars, starts with a letter, then alphanumeric or `-`
  (`utils.go:107-137`) — the regex is `^[a-zA-Z][a-zA-Z0-9\-]*[a-zA-Z0-9]$`;
- `name` ≤ 50 chars, `author` ≤ 25 chars;
- `version` is "basic semver";
- `language` ∈ `{go, javascript, typescript}` and `type` ∈ the five known
  provider/plugin types; plugin manifests get an extra version check
  (`utils.go:12-34`).

At load time, `extensionSanityCheck` additionally rejects an ID already in use,
and `semverConstraint` is checked with `Masterminds/semver` against the app
version; a mismatch records `invalid_semver_constraint` (prerelease app versions
skip the check) (`extension_repo/external.go:691-707`, `utils.go:92-105`).

Field-specific behavior:

- **`payload` vs `payloadURI`**: when fetching a remote manifest, if
  `payloadURI` is set the code is downloaded and inlined into `payload` before
  the manifest is stored, with a 10-second timeout and an empty-payload rejection
  (`external.go:38-95`, `97-123`). The only local `payloadURI` (a file path) is
  development mode, which **cannot be installed** for security reasons
  (`external.go:88-92`, `721-735`).
- **`permissions`** is a `[]string` explicitly commented `NOT IMPLEMENTED`
  (`extension.go:57-59`). The only enforced grant path is the *plugin* permission
  system (`external.go:741-759`, code `plugin_permissions_not_granted`).
  `InvalidExtensionAuthorizationError` (`invalid_authorization`) is declared but
  never assigned anywhere in the reference Go code (`extension.go:167-181`).
- **`userConfig`** declares `{version, requiresConfig, fields[]}` with
  `text`/`switch`/`select` fields, options and defaults (`extension.go:195-243`).
  Saved values live in a permanent filecache bucket keyed `ext_user_config_<id>`
  as `{version, values}` (`extension_repo/userconfig.go:12-14`, `210-215`).
  Missing config when required → `ErrMissingUserConfig`; a saved `version` that
  differs from the manifest's → `ErrIncompatibleUserConfig`. On load, each
  `{{field}}` placeholder in the payload is replaced with the saved value or the
  field default (`userconfig.go:21-80`). A config failure is recorded as
  `user_config_error` **but the extension is still loaded**
  (`external.go:765-780`); saving a config reloads it, uninstalling deletes it
  (`userconfig.go:128-163`).

### Repository / marketplace format and the install/update flow

- **Marketplace**: a flat JSON array of `Extension`; default URL is an obfuscated
  constant
  `https://raw.githubusercontent.com/5rahim/seanime-extensions/refs/heads/main/marketplace.json`
  (`internal/constants/constants.go:24`), overridable by the caller. Entries with
  no `id` or no `manifestURI` are dropped (`extension_repo/marketplace.go:14-50`).
- **Repository import**: `{"urls": ["<manifestURI>", ...]}`, passed either as a
  URL to fetch or as a JSON literal; every manifest is fetched concurrently,
  plugins are refused, and `install=false` gives a preview-only result
  (`external.go:204-295`). There is no signature or hash check on any fetch.
- **Install / update**: fetch manifest + payload, sanity check, write the whole
  manifest (with inlined payload) as `<extensionDir>/<id>.json`. If that file
  already exists it is deleted first and the operation is reported as an update;
  then the single extension is reloaded and its ID is removed from the cached
  update list (`external.go:133-193`). The directory defaults to
  `$SEANIME_DATA_DIR/extensions` (`internal/core/config.go:168`) and is created on
  startup (`repository.go:130-150`). `UpdateExtensionCode` offers a direct
  payload overwrite + reload path (`external.go:407-460`).
- **Update check**: a goroutine starts after the first external extension loads
  and repeats every 12 hours. For each loaded extension it re-fetches the
  manifest from its recorded `manifestURI`, skips builtins, sanity-checks it,
  refuses a changed ID, then flags any **string-inequality** of `version` as an
  `UpdateData` and emits the `ExtensionUpdatesFound` WebSocket event
  (`repository.go:157-179`, `external.go:342-401`, `events/events.go:63-64`).
  "Check for updates" can also force it synchronously (`repository.go:194-239`).
- **Uninstall**: read and delete `<id>.json`, asynchronously delete saved user
  config and (for plugins) plugin data, remove the ID from stored disabled
  settings, then reload — which notices the file is gone and emits
  `ExtensionsReloaded` (`external.go:301-336`).

### Running / broken extensions and error surfacing

`reloadExtension(id)` is the single choke point: delete from the extension bank,
delete the plugin pool, call `ClearInterrupt()` on the Goja VM (kills running
code), drop invalid/disabled entries, sleep 200 ms, load from the file if it
still exists, and send `ExtensionsReloaded` (`external.go:844-887`). A full
"reload all" first interrupts every external VM and unloads all external
extensions, then re-walks the directory (`external.go:466-641`). A runtime
failure can also mark an already-running VM invalid with `invalid_payload`
(`external.go:825-838`).

`InvalidExtensionErrorCode` covers `invalid_manifest`, `invalid_payload`,
`user_config_error`, `invalid_authorization` (unused),
`plugin_permissions_not_granted`, `invalid_semver_constraint`
(`extension.go:167-191`). `GetAllExtensions` splits fatal invalid extensions from
user-config ones and adds an `unsafeExtensions` map for plugin unsafe flags;
this is what the UI reads (`repository.go:194-239`). A manifest too corrupt to
parse is silently skipped rather than reported (`external_fs.go:9-24`).

### Playground

`RunPlaygroundCode` builds an in-memory `Extension` from `type`, `language`,
`code`, `inputs` and `function`, with a mock WebSocket manager and a debug logger
that buffers logs into a string (`extension_playground/playground.go:57-105`,
`109-156`). It runs the code through the same Goja provider constructors as an
installed extension (manga, onlinestream, or anime-torrent provider), calls the
selected function (`search`, `smartSearch`, `getTorrentInfoHash`, `findEpisodes`,
…), deletes the plugin pool afterward, and returns `{logs, value}` with the
value pretty-printed as JSON (`playground.go:190-495`). No manifest is required,
nothing is persisted, and this is separate from the install path's rejection of
development-mode extensions.

## mangayomi (Apache-2.0)

### Source format and repository index

A source is an Isar DB record with `id`, `name`, `baseUrl`, `lang`, `itemType`
(manga/anime/novel), `sourceCode`, `sourceCodeUrl`, `sourceCodeLanguage`
(dart/javascript/mihon/lnreader/aidoku), `version`, `versionLast`,
`appMinVerReq`, `headers`, `filterList`, `preferenceList`, icon/type flags,
`isAdded`, `isObsolete`, `isLocal`, `repo`, `updateRepo`, `updatedAt`
(`lib/models/source.dart:10-92`, `134-211`). A repository is just
`{name, website, jsonUrl, hidden}` (`lib/models/settings.dart:1744-1775`), stored
per item type on Settings (`settings.dart:259-263`;
`lib/providers/browse_state_provider.dart:119-189`). Repos are added by URL in a
settings screen: the validator requires an absolute `http(s)` URL and rejects
duplicates ignoring case/trailing slash; a URL without a file suffix is probed as
`index.min.json`, `repo.json`, `index.json`, `index_v2.json`
(`lib/modules/more/settings/browse/source_repositories.dart:239-318`;
`browse_state_provider.dart:234-275`). Repos can be hidden (excluded from
Browse) or removed; removal drops only *not-installed* listings
(`source_repositories.dart:197-237`).

`ExtensionStoreService.fetchStore` accepts several index shapes
(`lib/services/extension_store_service.dart:42-139`):

- binary protobuf `NetworkExtensionStore` (`.pb`), optionally with an external
  extension list URL (`extension_store_service.dart:141-197`);
- JSON `NetworkExtensionStore` (`meta.name`, `contact.website`,
  `extensionList.extensions` or `extensionListUrl`) (`:199-253`);
- legacy JSON array (`index.min.json`), following `repo.json` metadata and an
  `index_v2` redirect (`:340-587`);
- Aidoku `{"sources": [...]}` indexes (`:255-338`).

Entries are filtered by item type and by `appMinVerReq` against the running app
version (`lib/services/fetch_sources_list.dart:45-58`). Native Mangayomi sources
come from plain `Source.fromJson` objects whose code URL is `sourceCodeUrl`
(`extension_store_service.dart:524-537`; `source.dart:134-173`); Mihon APK and
Aidoku downloads are base64-encoded before being stored as `sourceCode`
(`fetch_sources_list.dart:313-318`). Source IDs are hashed strings such as
`'mihon-$pkg'.hashCode` or `'aidoku-$id-$lang'.hashCode.abs()`
(`extension_store_service.dart:323-800`), and the same ID is what ties a listing
to an installed source (`fetch_sources_list.dart:285`).

### Install, update and uninstall

- **Install** is `updateSource`: GET `sourceCodeUrl`, store the code (and
  headers/filter/preference metadata gathered through the matching runtime) on
  a Source row with `isAdded = true`, `version`/`versionLast` and `repo`
  (`fetch_sources_list.dart:306-393`). A direct install from a known listing URL
  skips reading all repository indexes (`lib/services/fetch_item_sources.dart:35-46`).
- **Update discovery**: all repos are read and offers are collected per ID.
  `chooseUpdate` prefers the newest build from the extension's **origin**
  repository; only if the origin has none does a newer build elsewhere count,
  and a cross-repo ("foreign") update is held as pending (`versionLast` +
  `updateRepo`) until the user accepts it. Auto-update applies only to own-repo
  offers (`lib/services/extension_update.dart:26-68`;
  `fetch_sources_list.dart:211-260`). Comparison is numeric by version part with
  prerelease handling; build metadata (`+127`) is ignored
  (`fetch_sources_list.dart:467-531`). A repository that moves is followed only
  when the *old URL* redirects to the new one — a matching name is not enough
  (`extension_update.dart:101-120`; `fetch_sources_list.dart:176-201`).
- **Update install**: `installExtensionUpdate` asks the pending update repo (or
  origin, or every repo when the code is missing), requires the same ID and a
  newer version, then calls `updateSource` and clears the pending markers
  (`fetch_sources_list.dart:266-304`).
- **Uninstall**: `sourceRepository.uninstall` deletes both preference value
  tables; obsolete/local sources are hard-deleted, while other sources keep the
  row with `sourceCode` cleared, `isAdded = false`, `isPinned = false` and
  `versionLast = version` (`lib/repositories/source_repository.dart:217-255`).
  "Obsolete" means no longer listed by its own repository
  (`fetch_sources_list.dart:423-465`).

### Running / broken extensions and error surfacing

Code lives in the DB; the Dart/JS runtime is a bounded isolate pool whose worker
`stop()` sends `dispose` and kills with `Isolate.immediate`
(`lib/services/isolate_service.dart:65-70`, `:73-89`). Updating overwrites the
row; no explicit per-extension VM restart keyed to an update was found in the
code read. A source whose code will not run falls back to empty filters,
preferences and "no Latest" rather than breaking a screen
(`test/services/broken_source_test.dart:8-29`).

Handled errors go through `recordError` into always-on crash reports and a
redacted log; `toastError` shows only a short, redacted first line and links to
the error-reports screen (`lib/utils/error_toast.dart:10-95`). Repository fetch
failures are logged and simply leave that repo's sources absent
(`fetch_item_sources.dart:73-79`). Source-code failures are collected in a
`SourceCodeErrors` notifier and shown as a `SourceCodeErrorBanner` on the
extension detail page and manga home, with log/debug output and dismissal on
success (`lib/services/source_code_errors.dart:25-69`;
`lib/modules/browse/widgets/source_code_error_banner.dart`). Library-update
failures are persisted as `UpdateError` records on Settings for a dedicated
screen (`lib/services/update_errors_provider.dart:9-42`). No hash or signature
verification of downloaded code was found in the install path.

## Cross-cutting notes for the Autumn decision

1. Both apps carry **origin identity** with each install and use it to decide
   whether an update is trusted: seanime by reusing the recorded `manifestURI`,
   mangayomi by storing `repo`/`updateRepo` and gating cross-repo updates on user
   consent.
2. Neither checks integrity of downloaded code. Their gates are schema/type
   validation, a version constraint against the app (`semverConstraint` /
   `appMinVerReq`), fetch timeouts and, in seanime, rejection of dev-mode
   manifests at install time.
3. Both persist a descriptor plus code and replace it atomically at update, then
   re-run/unload it. seanime explicitly interrupts the VM before reload;
   mangayomi relies on replacing the DB row and a worker pool.
4. seanime versions its user config and refuses to load a mismatched version;
   mangayomi stores preferences by source ID and wipes them on uninstall.
5. seanime's playground runs untrusted code in the same runtime but in memory,
   with mocked events and no persistence — a useful shape for Autumn's
   playground, while installs reject the equivalent dev-mode manifest.

Open questions these findings raise (not filed as tickets): whether Autumn wants
a payload hash/signature check (neither reference has one); whether a newer
extension from a different repository should require explicit consent (mangayomi
does this, seanime does not); and how a config-version mismatch should be
presented (seanime keeps the extension loaded but flags it).
