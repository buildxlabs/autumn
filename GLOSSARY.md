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
