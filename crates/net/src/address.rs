//! Classifies IP addresses the outbound guard refuses (ADR 0015).
//!
//! The tables hold every IANA IPv4 and IPv6 special-purpose block whose
//! _Globally Reachable_ column is `False` (registries updated 2025-10-09),
//! plus multicast, `240.0.0.0/4` and limited broadcast. Translation
//! prefixes are unpacked and the embedded IPv4 address is checked instead.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use AddressClass::{LinkLocal, Loopback, Multicast, Private, Reserved};
use ipnet::{Ipv4Net, Ipv6Net};

/// Why an address is not globally reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AddressClass {
    /// `127.0.0.0/8` or `::1`.
    Loopback,
    /// RFC 1918, shared CGNAT space and IPv6 unique-local addresses.
    Private,
    /// `169.254.0.0/16` and `fe80::/10`, which include cloud metadata endpoints.
    LinkLocal,
    /// `224.0.0.0/4` and `ff00::/8`.
    Multicast,
    /// Any other special-purpose, documentation, benchmarking or reserved block.
    Reserved,
}

impl AddressClass {
    /// Whether a LAN host exception given by name may reach this class.
    ///
    /// Link-local and multicast stay blocked unless the user enters the
    /// literal IP.
    pub(crate) fn lan_exception_may_reach(self) -> bool {
        matches!(self, Self::Loopback | Self::Private)
    }
}

impl fmt::Display for AddressClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link-local",
            Self::Multicast => "multicast",
            Self::Reserved => "reserved",
        })
    }
}

const fn v4(a: u8, b: u8, c: u8, d: u8, prefix: u8) -> Ipv4Net {
    Ipv4Net::new_assert(Ipv4Addr::new(a, b, c, d), prefix)
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors Ipv6Addr::new so the table reads like the registry"
)]
const fn v6(
    s0: u16,
    s1: u16,
    s2: u16,
    s3: u16,
    s4: u16,
    s5: u16,
    s6: u16,
    s7: u16,
    prefix: u8,
) -> Ipv6Net {
    Ipv6Net::new_assert(Ipv6Addr::new(s0, s1, s2, s3, s4, s5, s6, s7), prefix)
}

const BLOCKED_V4: &[(Ipv4Net, AddressClass)] = &[
    (v4(0, 0, 0, 0, 8), Reserved),
    (v4(10, 0, 0, 0, 8), Private),
    (v4(100, 64, 0, 0, 10), Private),
    (v4(127, 0, 0, 0, 8), Loopback),
    (v4(169, 254, 0, 0, 16), LinkLocal),
    (v4(172, 16, 0, 0, 12), Private),
    (v4(192, 0, 0, 0, 24), Reserved),
    (v4(192, 0, 2, 0, 24), Reserved),
    (v4(192, 88, 99, 0, 24), Reserved),
    (v4(192, 168, 0, 0, 16), Private),
    (v4(198, 18, 0, 0, 15), Reserved),
    (v4(198, 51, 100, 0, 24), Reserved),
    (v4(203, 0, 113, 0, 24), Reserved),
    (v4(224, 0, 0, 0, 4), Multicast),
    (v4(240, 0, 0, 0, 4), Reserved),
    (v4(255, 255, 255, 255, 32), Reserved),
];

const BLOCKED_V6: &[(Ipv6Net, AddressClass)] = &[
    (v6(0, 0, 0, 0, 0, 0, 0, 1, 128), Loopback),
    (v6(0, 0, 0, 0, 0, 0, 0, 0, 128), Reserved),
    (v6(0x100, 0, 0, 0, 0, 0, 0, 0, 64), Reserved),
    (v6(0x100, 0, 0, 1, 0, 0, 0, 0, 64), Reserved),
    (v6(0x2001, 0, 0, 0, 0, 0, 0, 0, 23), Reserved),
    (v6(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0, 32), Reserved),
    (v6(0x3fff, 0, 0, 0, 0, 0, 0, 0, 20), Reserved),
    (v6(0x5f00, 0, 0, 0, 0, 0, 0, 0, 16), Reserved),
    (v6(0xfc00, 0, 0, 0, 0, 0, 0, 0, 7), Private),
    (v6(0xfe80, 0, 0, 0, 0, 0, 0, 0, 10), LinkLocal),
    (v6(0xff00, 0, 0, 0, 0, 0, 0, 0, 8), Multicast),
];

/// IPv4-compatible addresses, `::a.b.c.d` (RFC 4291 §2.5.5.1).
const IPV4_COMPATIBLE: Ipv6Net = v6(0, 0, 0, 0, 0, 0, 0, 0, 96);
/// IPv4-mapped addresses, `::ffff:a.b.c.d`.
const IPV4_MAPPED: Ipv6Net = v6(0, 0, 0, 0, 0, 0xffff, 0, 0, 96);
/// Well-known NAT64 prefix (RFC 6052).
const NAT64: Ipv6Net = v6(0x64, 0xff9b, 0, 0, 0, 0, 0, 0, 96);
/// Local-use NAT64 prefix (RFC 8215). Only its `/96` form is unpacked.
const NAT64_LOCAL: Ipv6Net = v6(0x64, 0xff9b, 1, 0, 0, 0, 0, 0, 48);
/// 6to4 (RFC 3056): the IPv4 address sits in bits 16..48.
const SIX_TO_FOUR: Ipv6Net = v6(0x2002, 0, 0, 0, 0, 0, 0, 0, 16);

/// The class of a blocked address, or `None` for a globally reachable one.
pub(crate) fn classify(ip: IpAddr) -> Option<AddressClass> {
    match ip {
        IpAddr::V4(ip) => classify_v4(ip),
        IpAddr::V6(ip) => classify_v6(ip),
    }
}

fn classify_v4(ip: Ipv4Addr) -> Option<AddressClass> {
    BLOCKED_V4
        .iter()
        .find(|(net, _)| net.contains(&ip))
        .map(|&(_, class)| class)
}

fn classify_v6(ip: Ipv6Addr) -> Option<AddressClass> {
    let [s0, s1, s2, s3, s4, s5, s6, s7] = ip.segments();
    let low_v4 = Ipv4Addr::from((u32::from(s6) << 16) | u32::from(s7));
    // `::` and `::1` sit inside the IPv4-compatible block; the table wins.
    if s0 == 0 && s1 == 0 && s2 == 0 && s3 == 0 && s4 == 0 && s5 == 0 && s6 == 0 && s7 <= 1 {
        return classify_from_table(ip);
    }
    if IPV4_MAPPED.contains(&ip) || IPV4_COMPATIBLE.contains(&ip) || NAT64.contains(&ip) {
        return classify_v4(low_v4);
    }
    if NAT64_LOCAL.contains(&ip) {
        // Anything but the /96 layout embeds IPv4 where we cannot be sure.
        return if s3 == 0 && s4 == 0 && s5 == 0 {
            classify_v4(low_v4)
        } else {
            Some(Reserved)
        };
    }
    if SIX_TO_FOUR.contains(&ip) {
        return classify_v4(Ipv4Addr::from((u32::from(s1) << 16) | u32::from(s2)));
    }
    classify_from_table(ip)
}

fn classify_from_table(ip: Ipv6Addr) -> Option<AddressClass> {
    BLOCKED_V6
        .iter()
        .find(|(net, _)| net.contains(&ip))
        .map(|&(_, class)| class)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(table: &[(&str, Option<AddressClass>)]) {
        for &(input, want) in table {
            let ip: IpAddr = input.parse().unwrap();
            assert_eq!(classify(ip), want, "address {input}");
        }
    }

    #[test]
    fn every_blocked_ipv4_range_is_classified() {
        check(&[
            ("0.0.0.0", Some(Reserved)),
            ("0.255.255.255", Some(Reserved)),
            ("10.0.0.1", Some(Private)),
            ("10.255.255.255", Some(Private)),
            ("100.64.0.1", Some(Private)),
            ("100.127.255.255", Some(Private)),
            ("127.0.0.1", Some(Loopback)),
            ("127.255.255.254", Some(Loopback)),
            ("169.254.169.254", Some(LinkLocal)),
            ("172.16.0.1", Some(Private)),
            ("172.31.255.255", Some(Private)),
            ("192.0.0.8", Some(Reserved)),
            ("192.0.0.170", Some(Reserved)),
            ("192.0.2.1", Some(Reserved)),
            ("192.88.99.2", Some(Reserved)),
            ("192.168.1.1", Some(Private)),
            ("198.18.0.1", Some(Reserved)),
            ("198.19.255.255", Some(Reserved)),
            ("198.51.100.1", Some(Reserved)),
            ("203.0.113.1", Some(Reserved)),
            ("224.0.0.251", Some(Multicast)),
            ("239.255.255.255", Some(Multicast)),
            ("240.0.0.1", Some(Reserved)),
            ("255.255.255.255", Some(Reserved)),
        ]);
    }

    #[test]
    fn every_blocked_ipv6_range_is_classified() {
        check(&[
            ("::1", Some(Loopback)),
            ("::", Some(Reserved)),
            ("100::1", Some(Reserved)),
            ("100:0:0:1::1", Some(Reserved)),
            ("2001::1", Some(Reserved)),
            ("2001:2::1", Some(Reserved)),
            ("2001:1ff:ffff::1", Some(Reserved)),
            ("2001:db8::1", Some(Reserved)),
            ("3fff::1", Some(Reserved)),
            ("5f00::1", Some(Reserved)),
            ("fc00::1", Some(Private)),
            ("fd12:3456::1", Some(Private)),
            ("fe80::1", Some(LinkLocal)),
            ("febf::1", Some(LinkLocal)),
            ("ff02::fb", Some(Multicast)),
            ("ff0e::1", Some(Multicast)),
        ]);
    }

    #[test]
    fn embedded_ipv4_is_unpacked_and_rechecked() {
        check(&[
            // IPv4-mapped.
            ("::ffff:127.0.0.1", Some(Loopback)),
            ("::ffff:10.0.0.1", Some(Private)),
            ("::ffff:169.254.169.254", Some(LinkLocal)),
            ("::ffff:8.8.8.8", None),
            // IPv4-compatible.
            ("::127.0.0.1", Some(Loopback)),
            ("::192.168.0.1", Some(Private)),
            ("::8.8.8.8", None),
            // NAT64, well-known and local-use /96.
            ("64:ff9b::7f00:1", Some(Loopback)),
            ("64:ff9b::a00:1", Some(Private)),
            ("64:ff9b::808:808", None),
            ("64:ff9b:1::a00:1", Some(Private)),
            ("64:ff9b:1::808:808", None),
            ("64:ff9b:1:1::808:808", Some(Reserved)),
            // 6to4.
            ("2002:7f00:1::1", Some(Loopback)),
            ("2002:c0a8:101::1", Some(Private)),
            ("2002:808:808::1", None),
        ]);
    }

    #[test]
    fn globally_reachable_addresses_are_allowed() {
        check(&[
            ("1.1.1.1", None),
            ("8.8.8.8", None),
            ("93.184.216.34", None),
            ("100.63.255.255", None),
            ("100.128.0.0", None),
            ("172.15.255.255", None),
            ("172.32.0.0", None),
            ("223.255.255.255", None),
            ("2606:4700:4700::1111", None),
            ("2a00:1450:4001::200e", None),
            ("2001:4860:4860::8888", None),
        ]);
    }
}
