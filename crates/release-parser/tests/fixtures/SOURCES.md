# Corpus sources

`corpus.txt` holds real release names and the fields we expect `release_parser::parse` to read from them. Decision OD-1 (ROADMAP Phase 2) sets the sources and the review rule.

## Where the names come from

| Section in `corpus.txt`              | Names | Source                                                                                                                                                                             |
| ------------------------------------ | ----: | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `reference/seanime` tests            |   165 | File names in seanime's Go tests (`internal/library/**`, `internal/torrents/**`, …), GPL-3.0 like Autumn. A few use placeholder titles (`Title`, `Show`) but real naming patterns. |
| Nyaa, Anime - English-translated     |    45 | `https://nyaa.si/?page=rss&c=1_2&f=0`, fetched 2026-11-23                                                                                                                          |
| Nyaa, Anime - Non-English-translated |    35 | `https://nyaa.si/?page=rss&c=1_3&f=0`, fetched 2026-11-23                                                                                                                          |
| Nyaa, Anime - Raw                    |    28 | `https://nyaa.si/?page=rss&c=1_4&f=0`, fetched 2026-11-23                                                                                                                          |

The AnimeTosho feed (`https://animetosho.org/feed/rss2`) answered "Feed disabled" on 2026-11-23, so no names come from it yet. #114 retries it.

The feeds were read once by a throwaway script (download each feed, take every `<item><title>`, unescape HTML entities). The script is not in the repository and nothing in CI fetches anything.

## How names were picked and checked

- Only names in this ticket's scope: one episode, special, movie or extra per name. Episode ranges, batches and volumes (`01-12`, `S01E14-E26`, `Vol 1`) wait for #114.
- Names whose correct reading is unclear even to a person were left out rather than guessed at. Examples: `Part.2.05`, `Ranma 1_2` (an underscore standing in for `/`), and Chinese names that put the title inside a bracket.
- Every expected field was written by hand from reading the name. None was copied from another parser's output. anitomy's and habari's fixtures were not used (MPL-2.0, and not checked).
- The conventions the expectations follow are listed at the top of `corpus.txt`.
