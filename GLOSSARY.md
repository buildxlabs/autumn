# Autumn

A native desktop app for anime and manga: a local library, streaming, online sources from extensions, list tracking and a manga reader.

## Language

### Media and identity

**Media**:
One anime or manga as a catalog lists it, at the granularity of a single season or cour. Two seasons of the same story are two media joined by a relation.
_Avoid_: show, title, series, entry, franchise

**Media identity**:
Autumn's own identifier for a media. It exists once the user has recorded something about that media, and it never changes while the media lives.
_Avoid_: AniList id (as a synonym)

**External id**:
The identifier a provider (AniList, MyAnimeList, AniDB, Kitsu, or a custom source) gives a media. All providers count equally, and a media can have one external id per provider.
_Avoid_: source id, remote id

**Cross-reference**:
A link saying that an external id names a particular media. An external id belongs to at most one media.
_Avoid_: mapping, alias

**Custom source**:
A provider of media that are not in the AniList catalog, supplied by an extension.

### Library

**Local file**:
A video on disk that Autumn has found in a library folder and can play directly.
_Avoid_: media file, video file, entry

**Episode**:
A numbered installment of a media's main run, distinct from its specials and creditless clips.
_Avoid_: file, local file

**Special / NC**:
Video attached to a media but outside its episode numbering: an OVA, special, movie, NCOP or NCED.
_Avoid_: extra, episode

**Locked file**:
A local file whose match a re-scan never changes.
_Avoid_: pinned file, frozen file

**Ignored file**:
A local file a scan deliberately leaves out of the library.
_Avoid_: hidden file, excluded file

**Shelved file**:
A local file on a disconnected drive, kept with its match until the drive returns.
_Avoid_: missing file, offline file

**Scan summary**:
The outcome of one scan: files found, files matched and files left unmatched.
_Avoid_: report, scan log

### Extensions

**Declarative source**:
An extension made only of a manifest (URL templates and selectors) with no code.
_Avoid_: config extension, scraper definition

### Tracking

**List entry**:
A user's tracking record for one media on one platform: status, progress, score and the like.
_Avoid_: entry (on its own), list item

**Platform**:
Where a user's list entries live: the account-free local platform, or a tracking service account such as AniList or MyAnimeList.
_Avoid_: tracker, provider (a provider supplies external ids), service, backend

**Active platform**:
The one platform whose list entries are shown, edited and updated by playback right now. The other platforms' entries are dormant: kept, but untouched.
_Avoid_: current account, primary platform

**Local platform**:
The account-free platform. The user's list entries live only in Autumn.
_Avoid_: offline mode, guest account

**List transfer**:
A one-shot, user-reviewed copy of list entries from one platform to another. It creates or updates entries and never deletes.
_Avoid_: sync, merge, mirror, import, upload

**Custom list**:
A user-named list on a platform that a list entry can belong to besides its status. An entry can be in several.
_Avoid_: collection, folder, category

### Accounts

**AniList token**:
The credential that lets Autumn act on a user's AniList account.
_Avoid_: API key, session, password
