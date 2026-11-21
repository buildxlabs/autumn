# One active tracking platform; list entries are transferred, never mirrored

Exactly one platform (Local or AniList, later MyAnimeList) is active at a time, and each platform keeps its own list entries per `MediaId`. Moving entries between platforms is a list transfer: a one-shot copy the user reviews in a preview, which keeps the destination's entry on conflict by default and never deletes. We chose this over mirroring every write to all platforms because mirroring needs a permanent two-way conflict rule, makes AniList's account the user's backup only by accident, and pushes Autumn toward a "competing tracker" under AniList's terms (rule 5).

## Consequences

- While AniList is active, Local entries are dormant. Signing out brings them back exactly as they were, with an offer to transfer the AniList list into Local.
- An expired or revoked AniList token keeps AniList active, read-only, with a re-login prompt. It never falls back to Local.
- Local stores AniList's full list-entry field set (raw 0–100 score, custom lists, advanced scores, private, priority), so a transfer loses nothing.

## Considered options

- **Mirror to every linked platform** (mangayomi). Progress fans out to all trackers. Rejected for the conflict rule and the ToS exposure above.
- **AniList overwrites Local while active** (seanime's auto-sync). Simple, but it silently destroys Local edits.
- **Local always wins on upload** (seanime's manual upload). Overwrites years of AniList history without review.
