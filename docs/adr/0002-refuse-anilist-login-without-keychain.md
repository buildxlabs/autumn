# Refuse AniList login when no OS keychain is available

When the OS keychain is unavailable (Linux without a Secret Service daemon, or a locked or denying keychain), Autumn refuses the AniList login with a message that names the keychain problem, and the user stays on the local platform. The keychain is the only place Autumn keeps secrets (ROADMAP D6: tokens never land in SQLite or logs), and the fallbacks were worse: an encrypted file needs a passphrase that has nowhere safe to live, and a plaintext file with a warning protects nothing.

## Consequences

- A user on a machine without a keychain cannot connect AniList until one is set up. Account-free use (ROADMAP D7) keeps the app fully usable meanwhile.
- A corrupted stored token is treated as logged out, with a warning. The next login overwrites it.
- The underlying keychain error is logged at trace level. The token value never is.

## Considered options

- **Encrypted file with a user passphrase.** The passphrase has no storage of its own, so the user would type it on every launch.
- **Plaintext file with a warning.** No protection at all for a token that is valid for a year.
