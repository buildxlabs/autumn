# Fixture provenance

- `animetosho_search.json` is a recorded live response from
  `https://feed.animetosho.net/json?aid=17617&limit=3` on 2026-11-27, saved
  verbatim. It exercises `anidb_aid`, `is_seadex`, both infohash fields,
  magnet and torrent links, sizes and seeder/leecher counts.
- `nyaa_rss.xml` and `nyaa_html.html` transcribe the RSS `<item>` fields and
  HTML `table.torrent-list` columns observed on 2026-11-25 in
  `docs/research/nyaa-animetosho-search.md` (commit `d7766b6` on
  `origin/research/nyaa-animetosho-search`). A live capture from this host on
  2026-11-27 was not possible: `nyaa.si` reset the TLS connection and the
  suggested mirror returned 403. Names and hashes are illustrative; the
  structure, namespaces, row classes and field spellings are the observed
  ones.
