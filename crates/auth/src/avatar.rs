//! The animated-avatar probe from the AniList media CDN.

/// The path AniList uses for user avatars.
const AVATAR_PATH: &str = "file/anilistcdn/user/avatar/";

/// The animated GIF candidate for a static AniList avatar URL, if it has
/// one: `/medium/` becomes `/large/` and the extension becomes `.gif`.
///
/// Returns `None` for other hosts, other paths and URLs already ending in
/// `.gif`. This is the CDN path convention observed in seanime, not a
/// documented AniList field (see docs/research/anilist-api.md).
#[must_use]
fn gif_candidate(url: &str) -> Option<String> {
    let (_scheme, host_and_path) = url.split_once("://")?;
    let (host, path) = host_and_path.split_once('/')?;
    if host != "anilist.co" && !host.ends_with(".anilist.co") {
        return None;
    }
    let tail = path.strip_prefix(AVATAR_PATH)?;
    if tail.to_ascii_lowercase().ends_with(".gif") {
        return None;
    }
    let (base, _extension) = tail.rsplit_once('.')?;
    let large = base
        .strip_prefix("medium/")
        .map_or_else(|| base.to_owned(), |rest| format!("large/{rest}"));
    Some(format!("https://{host}/{AVATAR_PATH}{large}.gif"))
}

/// Returns the animated URL when AniList has a GIF for `url`, else `None`.
pub async fn animated(http: &net::Client, url: Option<&str>) -> Option<String> {
    let candidate = gif_candidate(url?)?;
    animated_at(http, &candidate).await.then_some(candidate)
}

/// Probes a candidate URL with a one-byte range request. `net` turns a 404
/// into an error, so a successful read means the GIF exists.
pub async fn animated_at(http: &net::Client, candidate: &str) -> bool {
    let Ok(request) = net::Request::new(net::Method::Get, candidate).header("Range", "bytes=0-0")
    else {
        return false;
    };
    http.read(request).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::gif_candidate;

    const MEDIUM: &str = "https://s4.anilist.co/file/anilistcdn/user/avatar/medium/u1-abc.jpg";

    #[test]
    fn a_medium_avatar_upgrades_to_a_large_gif() {
        assert_eq!(
            gif_candidate(MEDIUM).as_deref(),
            Some("https://s4.anilist.co/file/anilistcdn/user/avatar/large/u1-abc.gif")
        );
    }

    #[test]
    fn only_animated_candidates_are_built() {
        assert_eq!(
            gif_candidate("https://example.com/file/anilistcdn/user/avatar/medium/u1.jpg"),
            None
        );
        assert_eq!(
            gif_candidate("https://s4.anilist.co/file/other/medium/u1.jpg"),
            None
        );
        assert_eq!(
            gif_candidate("https://s4.anilist.co/file/anilistcdn/user/avatar/medium/u1.gif"),
            None
        );
        assert_eq!(gif_candidate("not a url"), None);
    }
}
