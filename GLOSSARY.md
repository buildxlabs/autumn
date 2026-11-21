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
Where a user's list entries live and sync: AniList, MyAnimeList, or the local platform. One platform is active at a time.
_Avoid_: tracker (for the destination), service, backend

**Local platform**:
The account-free platform. The user's list entries live only in Autumn.
_Avoid_: offline mode, guest account

### Accounts

**AniList token**:
The credential that lets Autumn act on a user's AniList account.
_Avoid_: API key, session, password
