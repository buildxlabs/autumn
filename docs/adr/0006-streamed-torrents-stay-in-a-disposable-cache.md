# Streamed torrents stay in a disposable cache

Streamed torrent files live in a disposable torrent cache for the lifetime of the app, not in library folders: when a file completes the relay switches to reading it from disk in place, and Autumn never moves, copies or hardlinks it into the library. Uploads continue while the app runs, but every torrent is dropped and its files deleted on quit, and nothing resumes on the next launch.

## Consequences

- Partial downloads stay out of library scans, and the cache avoids cross-restart state and per-torrent ratio accounting.
- Keeping an episode permanently is the downloader's job (Phase 6; the hardlink-to-library item stays in the Backlog), at the cost of re-downloading anything the user wanted to keep.
