//! Network settings → module wiring (#210 / ADR-0010, #225).
//!
//! The only Network fields that apply live are the global torrent rate
//! limits; listen port, upnp, DHT, torrent proxy and bound interface are
//! construction-time `SessionOptions`, so a change requires a torrent-module
//! restart. [`torrent_settings_change`] is a small pure decision the engine
//! calls on save and the engine's torrent module acts on; restart is
//! deferred while a stream is playing ("Applies after playback stops"). The global HTTP proxy rebuilds `net`'s
//! shared client immediately for new requests; indexer base URLs affect the
//! next search.

use settings::NetworkSettings;

/// Shown on the Network tab when a restart-type change can't apply yet.
pub const APPLIES_AFTER_PLAYBACK_STOPS: &str = "Applies after playback stops";

/// Whether a Network change requires restarting the torrent module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TorrentRestart {
    /// No construction-time torrent field changed.
    NotRequired,
    /// A construction-time field changed and no stream is playing: restart now.
    Now,
    /// A construction-time field changed but a stream is playing: restart
    /// when the last stream ends ("Applies after playback stops").
    AfterPlaybackStops,
}

/// The effect of a Network settings change on the torrent module (ADR-0010).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentSettingsChange {
    /// Restart requirement for construction-time torrent options.
    pub restart: TorrentRestart,
    /// Whether the global rate limits changed; applied live without restart.
    pub rate_limits_changed: bool,
    /// Current global torrent download limit (bytes/sec); `None` unlimited.
    pub download_limit_bytes_per_sec: Option<u32>,
    /// Current global torrent upload limit (bytes/sec); `None` unlimited.
    pub upload_limit_bytes_per_sec: Option<u32>,
}

/// Whether any construction-time torrent field differs. `settings`'s base
/// URLs, global HTTP proxy and other Network fields never restart torrents.
fn restart_relevant_changed(old: &NetworkSettings, new: &NetworkSettings) -> bool {
    old.listen_port != new.listen_port
        || old.upnp != new.upnp
        || old.dht != new.dht
        || !opt_eq(old.torrent_proxy.as_deref(), new.torrent_proxy.as_deref())
        || !opt_eq(old.bind_interface.as_deref(), new.bind_interface.as_deref())
}

/// Compares optional strings treating blank as unset and trimming.
fn opt_eq(a: Option<&str>, b: Option<&str>) -> bool {
    fn norm(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    norm(a) == norm(b)
}

/// The diff-and-restart decision the engine calls on Network save
/// (ADR-0010). `stream_playing` defers a restart until no stream is
/// playing; rate limits are reported separately for a live apply.
#[must_use]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "one flag for whether a stream is currently playing"
)]
pub fn torrent_settings_change(
    old: &NetworkSettings,
    new: &NetworkSettings,
    stream_playing: bool,
) -> TorrentSettingsChange {
    let restart = if !restart_relevant_changed(old, new) {
        TorrentRestart::NotRequired
    } else if stream_playing {
        TorrentRestart::AfterPlaybackStops
    } else {
        TorrentRestart::Now
    };
    TorrentSettingsChange {
        restart,
        rate_limits_changed: old.download_limit_bytes_per_sec != new.download_limit_bytes_per_sec
            || old.upload_limit_bytes_per_sec != new.upload_limit_bytes_per_sec,
        download_limit_bytes_per_sec: new.download_limit_bytes_per_sec,
        upload_limit_bytes_per_sec: new.upload_limit_bytes_per_sec,
    }
}

/// Construction-time torrent session options from Network settings
/// (ADR-0010); pair with [`torrent::PeerDiscovery::Public`].
#[must_use]
pub fn torrent_options(network: &NetworkSettings) -> torrent::TorrentSessionOptions {
    let listen_port = match network.listen_port {
        settings::ListenPort::Random => 0,
        settings::ListenPort::Fixed(port) => port,
    };
    let cleaned = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    torrent::TorrentSessionOptions {
        listen_port,
        upnp: network.upnp,
        dht: network.dht,
        proxy_url: cleaned(&network.torrent_proxy),
        bind_interface: cleaned(&network.bind_interface),
        rate_limits: rate_limits(network),
        // A Storage setting; the engine fills it from `settings.torrent`.
        disable_upload: false,
    }
}

/// The live torrent rate-limit application from Network settings; `None`
/// limits are unlimited. Applied to a running session without restart.
#[must_use]
pub fn rate_limits(network: &NetworkSettings) -> torrent::RateLimits {
    torrent::RateLimits::from_bytes_per_sec(
        network.download_limit_bytes_per_sec,
        network.upload_limit_bytes_per_sec,
    )
}

/// The `search` indexer configuration from Network settings (ADR-0010):
/// base URLs are normalized + validated upstream; `search` re-validates.
#[must_use]
pub fn search_config(network: &NetworkSettings) -> search::Config {
    search::Config::default()
        .with_nyaa_base_url(network.nyaa_base_url.clone())
        .with_animetosho_base_url(network.animetosho_base_url.clone())
}

#[cfg(test)]
mod tests {
    use settings::ListenPort;

    use super::*;

    fn base() -> NetworkSettings {
        NetworkSettings::default()
    }

    #[test]
    fn no_change_no_restart() {
        let a = base();
        let b = base();
        let change = torrent_settings_change(&a, &b, false);
        assert_eq!(change.restart, TorrentRestart::NotRequired);
        assert!(!change.rate_limits_changed);
    }

    #[test]
    fn rate_limit_only_change_is_live_not_restart() {
        let a = base();
        let b = NetworkSettings {
            download_limit_bytes_per_sec: Some(1_000_000),
            upload_limit_bytes_per_sec: Some(250_000),
            ..base()
        };
        for stream_playing in [false, true] {
            let change = torrent_settings_change(&a, &b, stream_playing);
            assert_eq!(change.restart, TorrentRestart::NotRequired);
            assert!(change.rate_limits_changed);
            assert_eq!(change.download_limit_bytes_per_sec, Some(1_000_000));
            assert_eq!(change.upload_limit_bytes_per_sec, Some(250_000));
        }
    }

    #[test]
    fn http_proxy_and_indexer_urls_do_not_restart_torrent() {
        let a = base();
        let b = NetworkSettings {
            http_proxy: Some("http://127.0.0.1:3128".to_owned()),
            nyaa_base_url: "https://mirror.example".to_owned(),
            animetosho_base_url: "https://feed.example".to_owned(),
            ..base()
        };
        let change = torrent_settings_change(&a, &b, false);
        assert_eq!(change.restart, TorrentRestart::NotRequired);
    }

    #[test]
    fn construction_time_changes_restart() {
        let variants = [
            NetworkSettings {
                listen_port: ListenPort::Fixed(6881),
                ..base()
            },
            NetworkSettings {
                upnp: true,
                ..base()
            },
            NetworkSettings {
                dht: false,
                ..base()
            },
            NetworkSettings {
                torrent_proxy: Some("socks5://127.0.0.1:1080".to_owned()),
                ..base()
            },
            NetworkSettings {
                bind_interface: Some("utun3".to_owned()),
                ..base()
            },
        ];
        for new in variants {
            let change = torrent_settings_change(&base(), &new, false);
            assert_eq!(change.restart, TorrentRestart::Now, "{new:?}");
        }
    }

    #[test]
    fn restart_is_deferred_while_a_stream_is_playing() {
        let a = base();
        let b = NetworkSettings {
            listen_port: ListenPort::Fixed(6881),
            ..base()
        };
        let change = torrent_settings_change(&a, &b, true);
        assert_eq!(change.restart, TorrentRestart::AfterPlaybackStops);
    }

    #[test]
    fn blank_optionals_match_unset() {
        let a = base();
        let b = NetworkSettings {
            torrent_proxy: Some("  ".to_owned()),
            bind_interface: Some(String::new()),
            ..base()
        };
        let change = torrent_settings_change(&a, &b, false);
        assert_eq!(change.restart, TorrentRestart::NotRequired);
    }

    #[tokio::test]
    async fn live_rate_limit_change_applies_without_restart() {
        // A real session; no running torrent module is needed to change
        // global rate limits live (ADR-0010).
        let cache = tempfile::tempdir().unwrap();
        let session = torrent::TorrentSession::start(
            cache.path().to_path_buf(),
            torrent::PeerDiscovery::Only(Vec::new()),
            torrent::TorrentSessionOptions::default(),
        )
        .await
        .unwrap();

        let old = base();
        let new = NetworkSettings {
            download_limit_bytes_per_sec: Some(1_000_000),
            upload_limit_bytes_per_sec: Some(500_000),
            ..base()
        };

        // The decision: rate-limit-only change, no restart, even while playing.
        let change = torrent_settings_change(&old, &new, true);
        assert_eq!(change.restart, TorrentRestart::NotRequired);
        assert!(change.rate_limits_changed);

        // Apply live; no session rebuild.
        session.set_rate_limits(rate_limits(&new));
        let applied = session.rate_limits();
        assert_eq!(
            applied.download_bytes_per_sec,
            std::num::NonZeroU32::new(1_000_000)
        );
        assert_eq!(
            applied.upload_bytes_per_sec,
            std::num::NonZeroU32::new(500_000)
        );

        session.shutdown().await.unwrap();
    }

    #[test]
    fn torrent_options_map_network_settings() {
        let network = NetworkSettings {
            listen_port: ListenPort::Fixed(6881),
            upnp: true,
            dht: false,
            torrent_proxy: Some("socks5://127.0.0.1:1080".to_owned()),
            bind_interface: Some("utun3".to_owned()),
            download_limit_bytes_per_sec: Some(1_000_000),
            ..base()
        };
        let options = torrent_options(&network);
        assert_eq!(options.listen_port, 6881);
        assert!(options.upnp);
        assert!(!options.dht);
        assert_eq!(
            options.proxy_url.as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
        assert_eq!(options.bind_interface.as_deref(), Some("utun3"));
        assert_eq!(
            options.rate_limits.download_bytes_per_sec,
            std::num::NonZeroU32::new(1_000_000)
        );

        let default = torrent_options(&base());
        assert_eq!(default.listen_port, 0, "random");
        assert!(!default.upnp);
        assert!(default.dht);
        assert_eq!(default.proxy_url, None);
        assert_eq!(default.bind_interface, None);
    }

    #[test]
    fn search_config_uses_network_base_urls() {
        let network = NetworkSettings {
            nyaa_base_url: "https://mirror.example".to_owned(),
            animetosho_base_url: "https://feed.example".to_owned(),
            ..base()
        };
        let config = search_config(&network);
        assert_eq!(config.nyaa_base_url, "https://mirror.example");
        assert_eq!(config.animetosho_base_url, "https://feed.example");

        let default = search_config(&NetworkSettings::default());
        assert_eq!(default, search::Config::default());
    }
}
