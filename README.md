# Autumn

A native desktop app for watching anime and reading manga, built in Rust with [GPUI](https://www.gpui.rs/) through [`gpui-kit`](https://gpui-kit.com).

Autumn puts every way you get an episode behind one interface:

- **Local library**: point it at your folders. It parses release filenames, matches them to shows, and keeps track as files come and go.
- **Torrent streaming**: start watching while the torrent downloads, or hand off to qBittorrent, Transmission or rqbit.
- **Debrid**: instant playback of cached torrents from Real-Debrid, TorBox, AllDebrid or Premiumize.
- **Online sources**: community extensions running in a sandbox.
- **Tracking**: AniList, MyAnimeList, or fully local with no account.
- **Manga**: library, providers and a built-in reader.

Video plays in mpv, IINA, VLC or MPC-HC, with resume, auto-next, intro skipping and automatic progress sync. An in-window player is planned.

> **Status:** early. The workspace and tooling exist; the app is being built phase by phase. See [ROADMAP.md](ROADMAP.md).

## Platforms

macOS (Apple silicon), Linux (x86_64, aarch64) and Windows. Desktop only.

## Development

Requirements: [Nix](https://nixos.org) with flakes and [direnv](https://direnv.net), or Rust 1.100, Node 24+ and pnpm installed yourself.

```sh
direnv allow        # or: nix develop
pnpm install        # git hooks + agent skills
cargo build --workspace   # `cargo run -p autumn` once Phase 0 lands
```

| Task              | Command          |
| ----------------- | ---------------- |
| Full check gate   | `pnpm check`     |
| Rust tests        | `pnpm test:rust` |
| Format everything | `pnpm fmts`      |
| Dependency audit  | `pnpm deny`      |

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md): crates, dependency rules, runtime model.
- [ROADMAP.md](ROADMAP.md): phases, decisions, feature coverage.
- [AGENTS.md](AGENTS.md): conventions for contributors and coding agents.
- [docs/research/](docs/research/): background research.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
