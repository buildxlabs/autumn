//! A Luau provider using the `autumn` host API end to end (#301).

use domain::ExtensionId;
use extensions::{ExtensionHost, ProviderRequest, ProviderSource, ProviderTorrent};

/// A Nyaa listing page recorded for the `search` crate's HTML tests.
const NYAA_HTML: &str = include_str!("fixtures/nyaa_html.html");

/// A Nyaa-style scraper: one `html.extract` over the listing's rows, then
/// its own size parser and `autumn.time.parse`.
const PROVIDER: &str = r#"
local units = { KiB = 1024, MiB = 1024 ^ 2, GiB = 1024 ^ 3, TiB = 1024 ^ 4 }

local function bytes(text)
    local number, unit = string.match(text or "", "([%d%.]+)%s*(%a+)")
    local scale = units[unit]
    if not number or not scale then return nil end
    return math.floor(tonumber(number) * scale)
end

return {
    search = function(request)
        local rows = autumn.html.extract(PAGE, "table.torrent-list tbody tr", {
            name = { css = "td:nth-child(2) a:last-child", attr = "title" },
            torrent_url = { css = "td:nth-child(3) a[href$='.torrent']", attr = "href" },
            magnet_url = { css = "td:nth-child(3) a[href^='magnet:']", attr = "href" },
            size = "td:nth-child(4)",
            published = "td:nth-child(5)",
            seeders = "td:nth-child(6)",
            leechers = "td:nth-child(7)",
            downloads = "td:nth-child(8)",
        })
        local results = {}
        for _, row in rows do
            table.insert(results, {
                name = row.name,
                magnet_url = row.magnet_url,
                torrent_url = row.torrent_url
                    and autumn.url.resolve("https://nyaa.si/", row.torrent_url),
                size = bytes(row.size),
                published = row.published and autumn.time.parse(row.published),
                seeders = tonumber(row.seeders),
                leechers = tonumber(row.leechers),
                downloads = tonumber(row.downloads),
            })
        end
        return results
    end,
}
"#;

#[tokio::test]
async fn a_scraper_extracts_rows_from_a_recorded_listing() {
    // The page is inlined so this test needs no server; tests/http.rs fetches.
    let source = format!("local PAGE = [=====[{NYAA_HTML}]=====]\n{PROVIDER}");
    let host = ExtensionHost::start(
        vec![ProviderSource {
            id: ExtensionId::new("nyaa-html"),
            name: "Nyaa (HTML)".to_owned(),
            version: "1.0.0".to_owned(),
            source,
            hosts: Vec::new(),
        }],
        &net::GuardedClient::new().unwrap(),
    );
    let rows = host
        .search(
            &ExtensionId::new("nyaa-html"),
            ProviderRequest::keyword("frieren"),
        )
        .await
        .unwrap();

    let row = |name: &str, id: u32, hash: char, size: f64, published: f64, counts: [f64; 3]| {
        ProviderTorrent {
            name: Some(name.to_owned()),
            magnet_url: Some(format!(
                "magnet:?xt=urn:btih:{}&dn=Frieren",
                hash.to_string().repeat(40)
            )),
            torrent_url: Some(format!("https://nyaa.si/download/{id}.torrent")),
            size: Some(size),
            published: Some(published),
            seeders: Some(counts[0]),
            leechers: Some(counts[1]),
            downloads: Some(counts[2]),
            ..ProviderTorrent::default()
        }
    };
    assert_eq!(
        rows,
        vec![
            row(
                "[SubsPlease] Sousou no Frieren - 01 (1080p) [ABCD1234].mkv",
                2_000_001,
                'a',
                342_255_206.0,
                1_795_521_600.0,
                [120.0, 15.0, 3400.0],
            ),
            row(
                "[OldGroup] Sousou no Frieren - 01 (720p) [DEADBEEF].mkv",
                2_000_002,
                'b',
                1_288_490_188.0,
                1_795_527_000.0,
                [3.0, 1.0, 42.0],
            ),
        ]
    );
}
