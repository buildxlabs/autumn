# Corpus sources

`corpus.txt` holds real release names and the fields we expect `release_parser::parse` to read from them. Decision OD-1 (ROADMAP Phase 2) sets the sources and the review rule.

## Where the names come from

| Section in `corpus.txt`              | Names | Source                                                                                                                                                                                                                                            |
| ------------------------------------ | ----: | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `reference/seanime` tests            |   164 | File names in seanime's Go tests (`internal/library/**`, `internal/torrents/**`, …), GPL-3.0 like Autumn. A few use placeholder titles (`Title`, `Show`) but real naming patterns.                                                                |
| Nyaa, Anime - English-translated     |    45 | `https://nyaa.si/?page=rss&c=1_2&f=0`, fetched 2026-11-23                                                                                                                                                                                         |
| Nyaa, Anime - Non-English-translated |    35 | `https://nyaa.si/?page=rss&c=1_3&f=0`, fetched 2026-11-23                                                                                                                                                                                         |
| Nyaa, Anime - Raw                    |    28 | `https://nyaa.si/?page=rss&c=1_4&f=0`, fetched 2026-11-23                                                                                                                                                                                         |
| Nyaa searches for kind words         |    18 | `https://nyaa.si/?page=rss&c=1_0&f=0&q=…` for `Gekijouban`, `NCOP`, `PV`, `ONA`, `OVA` and `Menu`, fetched 2026-11-24, so every kind has real names                                                                                               |
| Nyaa searches for batches and more   |   214 | `https://nyaa.si/?page=rss&c=1_0&f=0&q=…` for `01-12`, `batch`, `complete`, `Vol`, `S01-S0`, `One Piece`, `Movie`, `BDRip`, `Raws`, and the groups `Judas`, `EMBER`, `ASW`, `Okay-Subs`, `SubsPlease 1080p`, `Erai-raws HEVC`, fetched 2026-11-24 |

The AnimeTosho feed (`https://animetosho.org/feed/rss2`) answered "Feed disabled" on 2026-11-23 and again on 2026-11-24, so no names come from it. Nyaa carries the same releases, so the corpus loses little.

In total: 504 names from 99 release groups, with absolute numbering (`- 1221`, `EP1160`), multi-season packs, movies, raws (TV captures, `.ts` recordings) and Chinese, Japanese, Korean, Portuguese and French names.

The feeds were read once by a throwaway script (download each feed, take every `<item><title>`, unescape HTML entities). The script is not in the repository and nothing in CI fetches anything.

## How names were picked and checked

- The #113 sections hold single episodes, specials, movies and extras. The #114 section adds episode and season ranges, batches and volumes.
- From the #114 section on, every name also states its technical tags (or `tags: none`). The older names are checked on everything but tags; the table tests in `tests/tags.rs` cover each tag family.
- Names whose correct reading is unclear even to a person were left out rather than guessed at. Examples: `Part.2.05`, `Ranma 1_2` (an underscore standing in for `/`), and Chinese names that put the title inside a bracket, names whose group is a trailing bracket (`Title ONA 01 [Group]`), which the parser doesn't read, and multi-part packs such as `S01-S02+OVA` or `TV + Movie`.
- Every expected field was written by hand from reading the name. None was copied from another parser's output. anitomy's and habari's fixtures were not used (MPL-2.0, and not checked).
- The conventions the expectations follow are listed at the top of `corpus.txt`.
