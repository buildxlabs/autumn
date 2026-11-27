//! Settings → Network: defaults, validation and the database row shape.

use settings::{ListenPort, NetworkSettings, Settings, SettingsError};

fn network() -> NetworkSettings {
    NetworkSettings::default()
}

fn invalid(settings: &NetworkSettings, field: &str) {
    match settings.validate() {
        Err(SettingsError::Validation { field: got, .. }) => {
            assert_eq!(got, field, "wrong field flagged");
        }
        other => {
            assert!(
                matches!(other, Err(SettingsError::Validation { .. })),
                "expected validation error on {field}, got {other:?}"
            );
        }
    }
}

#[test]
fn defaults_are_adr_0010_policy() {
    let settings = network();

    assert_eq!(
        settings.listen_port,
        ListenPort::Random,
        "random per session"
    );
    assert!(!settings.upnp, "upnp off");
    assert!(settings.dht, "DHT on");
    assert_eq!(settings.download_limit_bytes_per_sec, None, "unlimited");
    assert_eq!(settings.upload_limit_bytes_per_sec, None, "unlimited");
    assert_eq!(settings.torrent_proxy, None);
    assert_eq!(settings.bind_interface, None);
    assert_eq!(settings.nyaa_base_url, "https://nyaa.si");
    assert_eq!(settings.animetosho_base_url, "https://feed.animetosho.net");
    assert_eq!(settings.http_proxy, None);
    assert!(settings.validate().is_ok());
}

#[test]
fn fixed_listen_port_range_is_1024_to_65535() {
    let mut settings = network();

    settings.listen_port = ListenPort::Fixed(1023);
    invalid(&settings, "network.listen_port");
    settings.listen_port = ListenPort::Fixed(0);
    invalid(&settings, "network.listen_port");

    settings.listen_port = ListenPort::Fixed(1024);
    assert!(settings.validate().is_ok(), "inclusive lower bound");
    settings.listen_port = ListenPort::Fixed(65535);
    assert!(settings.validate().is_ok(), "inclusive upper bound");
    settings.listen_port = ListenPort::Fixed(6881);
    assert!(settings.validate().is_ok());
}

#[test]
fn zero_rate_limits_are_rejected() {
    let mut settings = network();

    settings.download_limit_bytes_per_sec = Some(0);
    invalid(&settings, "network.download_limit_bytes_per_sec");
    settings.download_limit_bytes_per_sec = None;
    settings.upload_limit_bytes_per_sec = Some(0);
    invalid(&settings, "network.upload_limit_bytes_per_sec");
    settings.upload_limit_bytes_per_sec = Some(1);
    assert!(settings.validate().is_ok());
}

#[test]
fn torrent_proxy_must_be_socks5_with_host_port_and_no_credentials() {
    let mut settings = network();

    settings.torrent_proxy = Some("http://proxy.local:1080".to_owned());
    invalid(&settings, "network.torrent_proxy");

    settings.torrent_proxy = Some("socks5://proxy.local".to_owned());
    invalid(&settings, "network.torrent_proxy");

    settings.torrent_proxy = Some("socks5://user:pass@proxy.local:1080".to_owned());
    invalid(&settings, "network.torrent_proxy");

    settings.torrent_proxy = Some("socks5://user@proxy.local:1080".to_owned());
    invalid(&settings, "network.torrent_proxy");

    settings.torrent_proxy = Some("not a url".to_owned());
    invalid(&settings, "network.torrent_proxy");

    settings.torrent_proxy = Some("socks5://proxy.local:1080".to_owned());
    assert!(settings.validate().is_ok());
    settings.torrent_proxy = Some("socks5h://proxy.local:1080".to_owned());
    assert!(
        settings.validate().is_ok(),
        "socks5h resolves hostnames via proxy"
    );
}

#[test]
fn http_proxy_is_http_or_socks5_with_no_embedded_credentials() {
    let mut settings = network();

    settings.http_proxy = Some("socks4://proxy.local:1080".to_owned());
    invalid(&settings, "network.http_proxy");

    settings.http_proxy = Some("http://user:pass@proxy.local:3128".to_owned());
    invalid(&settings, "network.http_proxy");

    settings.http_proxy = Some("http://user@proxy.local:3128".to_owned());
    invalid(&settings, "network.http_proxy");

    settings.http_proxy = Some("http://proxy.local:3128".to_owned());
    assert!(settings.validate().is_ok());
    settings.http_proxy = Some("socks5://proxy.local:1080".to_owned());
    assert!(settings.validate().is_ok());
}

#[test]
fn indexer_base_urls_are_normalized_and_validated() {
    let mut settings = network();

    // Bare host normalizes to https.
    settings.nyaa_base_url = "nyaa.si".to_owned();
    assert!(settings.validate().is_ok());
    let normalized = settings.normalized().unwrap();
    assert_eq!(normalized.nyaa_base_url, "https://nyaa.si/");

    // Mirror host + scheme http allowed.
    settings.nyaa_base_url = "http://mirror.local".to_owned();
    assert!(settings.validate().is_ok());

    // No userinfo.
    settings.nyaa_base_url = "https://user@nyaa.si".to_owned();
    invalid(&settings, "network.nyaa_base_url");
    settings.nyaa_base_url = "https://user:pass@nyaa.si".to_owned();
    invalid(&settings, "network.nyaa_base_url");

    // No path/query/fragment; scheme required.
    settings.nyaa_base_url = "https://nyaa.si/foo".to_owned();
    invalid(&settings, "network.nyaa_base_url");
    settings.nyaa_base_url = "https://nyaa.si/?x=1".to_owned();
    invalid(&settings, "network.nyaa_base_url");
    settings.nyaa_base_url = "https://nyaa.si/#frag".to_owned();
    invalid(&settings, "network.nyaa_base_url");
    settings.nyaa_base_url = "ftp://nyaa.si".to_owned();
    invalid(&settings, "network.nyaa_base_url");
    settings.nyaa_base_url = String::new();
    invalid(&settings, "network.nyaa_base_url");
}

#[test]
fn animetosho_base_url_is_validated_independently() {
    let mut settings = network();
    settings.animetosho_base_url = "https://feed.animetosho.org".to_owned();
    assert!(settings.validate().is_ok());
    settings.animetosho_base_url = "https://user@feed.animetosho.net".to_owned();
    invalid(&settings, "network.animetosho_base_url");
}

#[test]
fn network_round_trips_through_settings_rows() {
    let settings = Settings {
        network: NetworkSettings {
            listen_port: ListenPort::Fixed(6881),
            upnp: true,
            dht: false,
            download_limit_bytes_per_sec: Some(1_000_000),
            upload_limit_bytes_per_sec: Some(500_000),
            torrent_proxy: Some("socks5://127.0.0.1:1080".to_owned()),
            bind_interface: Some("utun3".to_owned()),
            nyaa_base_url: "https://nyaa.si".to_owned(),
            animetosho_base_url: "https://feed.animetosho.net".to_owned(),
            http_proxy: Some("http://127.0.0.1:3128".to_owned()),
        },
        ..Settings::default()
    };
    let rows = settings.to_rows().unwrap();

    let (loaded, errors) = Settings::from_rows(rows);

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded.network, settings.network);
}

#[test]
fn missing_network_tab_takes_defaults() {
    let (loaded, errors) = Settings::from_rows(Vec::<(String, String)>::new());
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded.network, NetworkSettings::default());
}
