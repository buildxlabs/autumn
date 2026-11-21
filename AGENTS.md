# AGENTS.md

Autumn is a native desktop app for anime and manga, written in Rust with GPUI (through `gpui-kit`). It brings together a local file library, torrent and debrid streaming, online sources from sandboxed extensions, AniList/MAL/account-free tracking, and a manga reader. Video plays in mpv (and similar players). Desktop only: no web app, no server mode.

## Where things are

- [ARCHITECTURE.md](ARCHITECTURE.md): crate map, dependency rules, GPUI ↔ Tokio bridge. Read it before adding a crate, a dependency edge, or anything that crosses the UI/engine boundary.
- [ROADMAP.md](ROADMAP.md): phases, exit criteria, open decisions, feature coverage matrix. Read it before starting feature work, to find the phase and its exit criterion.
- `docs/research/`: the original research. Background only. ARCHITECTURE and ROADMAP win where they differ.
- `reference/`: third-party apps, read-only, excluded from every tool. Study them for **behavior**. Autumn is GPL-3.0-only, so code from GPL-3.0, Apache-2.0 and MIT references may be ported: keep the original copyright and license notice at the top of the ported file. AGPL-3.0 references (`reiverr`, `bookorbit`) are behavior-only.

## Environment

- Toolchain: `direnv allow` (or `nix develop`) gives the pinned Rust, pnpm, taplo, typos, cargo-deny and cargo-nextest.
- Run `pnpm install` once per clone. It installs the git hooks and links the skills below into `.agents/skills` and `.claude/skills`. Without it, no skill exists.
- Commands live in `package.json` scripts. Before reporting a change done: `pnpm fmts`, `pnpm check`, `pnpm test:rust`.

## Rust rules the config doesn't spell out

- **GPUI stays in the UI.** Only `ui-*` and the `autumn` binary depend on `gpui-kit`. Back-end crates expose async functions and plain types.
- **Allowed crate edges are listed in ARCHITECTURE §3.** A new edge changes that list in the same commit.
- **Versions live in root `[workspace.dependencies]`.** Crates write `foo.workspace = true`.
- **Fallible code returns `Result`.** The workspace lints deny `unwrap`, `expect`, `panic`, indexing and printing outside tests. Use `get()`, `?`, a `thiserror` enum per crate, and `tracing` for output.
- **Suppress lints with `#[expect(lint, reason = "…")]`.** `allow` attributes are denied.
- **`render()` is pure.** I/O goes through `bridge::run`. Store the returned `Task` on the entity so a newer request cancels the older one.

## Skills

The agent loads these on its own when the task matches:

| Skill                 | Reach for it when                                                                                  |
| --------------------- | -------------------------------------------------------------------------------------------------- |
| `rust-best-practices` | Writing or changing any Rust, `Cargo.toml`, tests or rustdoc. This is almost every task here.      |
| `rust-review`         | Reviewing or auditing Rust code, a diff or a PR.                                                   |
| `rust-port`           | Re-implementing behavior seen in another language in Rust (mind the license rule above).           |
| `karpathy-guidelines` | Writing, reviewing or refactoring code. Same rules as the section below.                           |
| `tdd`                 | Building a feature or fixing a bug test-first. Default for `release-parser`, matching and scoring. |
| `code-review`         | Reviewing changes since a commit, branch or merge-base against standards and the roadmap.          |
| `domain-modeling`     | Naming domain concepts, editing a glossary, or recording an ADR.                                   |
| `research`            | Collecting facts from primary sources (crate docs, API specs) into a Markdown file in the repo.    |
| `grilling`            | Stress-testing a plan or decision before building it.                                              |
| `pr`                  | Writing a pull request body.                                                                       |
| `writing-for-agents`  | Editing this file, `CLAUDE.md`, or any skill.                                                      |

These run only when the user types the slash command (`disable-model-invocation` is set upstream):

| Command                     | Does                                                                   |
| --------------------------- | ---------------------------------------------------------------------- |
| `/grill-me`                 | Interviews the user to sharpen a plan.                                 |
| `/grill-with-docs`          | The same interview, writing ADRs and glossary entries as it goes.      |
| `/to-spec`                  | Turns the conversation into a spec on the issue tracker.               |
| `/to-tickets`               | Splits a plan into tracer-bullet tickets with blocking edges.          |
| `/implement`                | Implements a spec or a set of tickets.                                 |
| `/wayfinder`                | Maps work too big for one session as decision tickets.                 |
| `/triage`                   | Moves issues and external PRs through triage.                          |
| `/handoff`                  | Compacts the session into a handoff doc for the next agent.            |
| `/setup-matt-pocock-skills` | One-time setup of tracker, labels and doc layout for the skills above. |

### Skill configuration

The engineering skills read these files:

- **Issue tracker**: GitHub Issues in `buildxlabs/autumn`, through the `gh` CLI. See `docs/agents/issue-tracker.md`.
- **Triage labels**: the canonical names, unchanged. See `docs/agents/triage-labels.md`.
- **Domain docs**: single-context (one `GLOSSARY.md`, ADRs in `docs/adr/`). See `docs/agents/domain.md`.

## Commits

Every commit has a Conventional Commits title and a body, each passed with its own `-m` flag:

```sh
git commit -m "feat(library): shelve files on disconnected drives" \
  -m "Scanning while an external drive is unplugged used to delete its matched
files. Locked files on a missing mount are now marked shelved and keep their
match until the drive returns.

Refs ROADMAP Phase 2."
```

- **Title:** `type(scope): summary`, imperative mood, at most 72 characters, no trailing period. Types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`, `ci`, `chore`. Scope is the crate name, or `workspace`, `nix`, `docs` or `tooling`.
- **Body:** what changed and why, wrapped at 72 columns. Name the roadmap phase or decision it serves. Note anything a reviewer can't see from the diff (a skipped check, a follow-up).
- One logical change per commit. Formatting-only changes go in their own commit.
- Commit only when the user asks. On `main`, create a branch first.

## Behavioral guidelines

These guidelines bias toward caution over speed. For trivial tasks, use judgment.

### 1. Think before coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:

- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them. Don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

### 2. Simplicity first

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

### 3. Surgical changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:

- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it. Don't delete it.

When your changes create orphans:

- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: every changed line should trace directly to the user's request.

### 4. Goal-driven execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:

- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:

```text
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

These guidelines are working if diffs contain fewer unnecessary changes, fewer rewrites come from overcomplication, and clarifying questions come before implementation rather than after mistakes.
