# Torrent streams reach the player only through a per-stream token-gated relay

The player is always given a loopback relay URL for a torrent stream, never a file path, even after the file completes. The relay switches to reading the file from disk in place, and each stream's 128-bit token is revoked when the playback session ends. We chose this over switching the player to the local file with `loadfile`: that reopens and seeks the file mid-episode, and VLC/MPC-HC can only relaunch.

## Consequences

- A token per stream, instead of one per app run, makes ending a session cut the connection cleanly, and a stale URL in the player's history stops working.
- The relay serves every byte of a streamed episode for the whole session, including after the file is complete.

## Considered options

- **Switch the player to the local file with `loadfile`.** It reopens and seeks mid-episode; VLC/MPC-HC can only relaunch.
- **One token per app run.** Ending a session would not cut the connection cleanly.
