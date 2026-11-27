# An incoming torrent must be matched to a Media and Episode before it can stream

User-supplied torrents arrive by magnet paste, `/magnet`, `/droptorrent` or drop. Continuity, watched threshold, resume and progress sync are keyed by (media, episode) for every source, and indexer auto-select refuses to invent a match (ADR-0007). We never auto-commit a match: a review step shows the best guess, a ranked list or a manual pick, and a torrent without a match can only be added to the Torrent List — playing it later routes back through review.

## Consequences

- Matching costs one confirmation click and a metadata fetch before a magnet's file can be picked.
- A wrong automatic match would write resume state against no real media; the review step keeps continuity coherent.

## Considered options

- **Trust the magnet display name and auto-attach.** It writes resume state against a guessed media and episode.
- **Allow anonymous, untracked playback.** It would need an "untracked" source variant and the same bad resume state.
