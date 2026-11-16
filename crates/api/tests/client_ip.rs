use std::net::IpAddr;

use api::client_ip::resolve;
use ipnet::IpNet;
use proptest::prelude::*;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn proxies() -> Vec<IpNet> {
    vec!["10.0.0.0/8".parse().unwrap(), "fd00::/8".parse().unwrap()]
}

#[test]
fn untrusted_peer_cannot_set_its_address() {
    assert_eq!(
        resolve(ip("203.0.113.9"), "1.2.3.4", &proxies()),
        ip("203.0.113.9")
    );
    assert_eq!(
        resolve(ip("203.0.113.9"), "1.2.3.4", &[]),
        ip("203.0.113.9")
    );
}

#[test]
fn trusted_peer_without_header_is_the_client() {
    assert_eq!(resolve(ip("10.0.0.1"), "", &proxies()), ip("10.0.0.1"));
}

#[test]
fn rightmost_untrusted_hop_is_the_client() {
    let p = proxies();
    assert_eq!(
        resolve(ip("10.0.0.1"), "198.51.100.7", &p),
        ip("198.51.100.7")
    );
    // The client prepended a fake address; the proxy appended the real one.
    assert_eq!(
        resolve(ip("10.0.0.1"), "6.6.6.6, 198.51.100.7", &p),
        ip("198.51.100.7")
    );
    // Two proxies in the chain.
    assert_eq!(
        resolve(ip("10.0.0.1"), "6.6.6.6, 198.51.100.7, 10.0.0.2", &p),
        ip("198.51.100.7")
    );
}

#[test]
fn all_hops_trusted_gives_the_leftmost() {
    assert_eq!(
        resolve(ip("10.0.0.1"), "10.1.1.1, 10.2.2.2", &proxies()),
        ip("10.1.1.1")
    );
}

#[test]
fn garbage_is_not_guessed_at() {
    let p = proxies();
    assert_eq!(resolve(ip("10.0.0.1"), "nonsense", &p), ip("10.0.0.1"));
    assert_eq!(
        resolve(ip("10.0.0.1"), "6.6.6.6, nonsense", &p),
        ip("10.0.0.1")
    );
    assert_eq!(
        resolve(ip("10.0.0.1"), "198.51.100.7, 10.0.0.2, ", &p),
        ip("10.0.0.1")
    );
}

#[test]
fn ipv4_mapped_ipv6_matches_ipv4_rules() {
    let p = proxies();
    assert_eq!(
        resolve(ip("::ffff:10.0.0.1"), "198.51.100.7", &p),
        ip("198.51.100.7")
    );
    assert_eq!(
        resolve(ip("::ffff:203.0.113.9"), "1.2.3.4", &p),
        ip("203.0.113.9")
    );
}

#[test]
fn ipv6_clients_behind_ipv6_proxies() {
    assert_eq!(
        resolve(ip("fd00::1"), "2001:db8::5", &proxies()),
        ip("2001:db8::5")
    );
}

proptest! {
    #[test]
    fn untrusted_peers_are_always_themselves(peer: u32, header in ".{0,80}") {
        let peer = IpAddr::from(peer.to_be_bytes());
        prop_assume!(!proxies().iter().any(|n| n.contains(&peer)));
        prop_assert_eq!(resolve(peer, &header, &proxies()), peer);
    }

    #[test]
    fn never_panics_and_never_returns_a_spoofed_left_entry(spoof: u32, real: u32) {
        let (spoof, real) = (IpAddr::from(spoof.to_be_bytes()), IpAddr::from(real.to_be_bytes()));
        prop_assume!(!proxies().iter().any(|n| n.contains(&real)));
        let header = format!("{spoof}, {real}");
        prop_assert_eq!(resolve(ip("10.0.0.1"), &header, &proxies()), real);
    }
}
