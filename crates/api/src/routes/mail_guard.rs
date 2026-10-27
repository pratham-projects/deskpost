//! SSRF guard for the SMTP/IMAP hosts an admin can configure. The server dials these hosts (mail
//! worker, connection tests), so by default they must not point at loopback, private, link-local or
//! otherwise internal addresses. `SD_ALLOW_PRIVATE_MAIL_HOSTS=1` lifts the address rule (dev, e2e).

use std::net::IpAddr;

use ipnet::IpNet;

/// Reserved IPv4 ranges not covered by the std `is_*` helpers.
const BLOCKED_V4: [&str; 8] = [
    "0.0.0.0/8",
    "100.64.0.0/10",   // carrier-grade NAT
    "192.0.0.0/24",    // IETF protocol assignments
    "192.0.2.0/24",    // documentation
    "198.18.0.0/15",   // benchmarking
    "198.51.100.0/24", // documentation
    "203.0.113.0/24",  // documentation
    "240.0.0.0/4",     // reserved + broadcast
];
const BLOCKED_V6: [&str; 5] = [
    "::/128",
    "fc00::/7",      // unique local
    "fe80::/10",     // link local
    "fec0::/10",     // deprecated site local
    "2001:db8::/32", // documentation
];
/// Internal-only name suffixes; a host that has no dot at all is also internal.
const INTERNAL_SUFFIXES: [&str; 5] = [".localhost", ".local", ".internal", ".localdomain", ".lan"];

pub fn is_blocked_ip(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || BLOCKED_V4
                    .iter()
                    .any(|n| n.parse::<IpNet>().is_ok_and(|n| n.contains(&ip)))
        }
        IpAddr::V6(v6) => {
            // NAT64 (64:ff9b::/96) carries an IPv4 address in the last 32 bits.
            let o = v6.octets();
            let nat64 = o[..12] == [0, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0];
            if nat64 {
                return is_blocked_ip(IpAddr::from([o[12], o[13], o[14], o[15]]));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || BLOCKED_V6
                    .iter()
                    .any(|n| n.parse::<IpNet>().is_ok_and(|n| n.contains(&ip)))
        }
    }
}

/// Syntax and static policy check, no DNS. Returns the normalised (lower-case, no trailing dot) host.
pub fn check_host(host: &str, allow_private: bool) -> Result<String, &'static str> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return Err("Enter a valid host name.");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if allow_private || !is_blocked_ip(ip) {
            Ok(host)
        } else {
            Err("This address is not allowed (loopback or private network).")
        };
    }
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !labels.iter().all(label_ok) {
        return Err("Enter a valid host name.");
    }
    // `127.1`, `2130706433` and `0x7f.1` are IP spellings that resolvers accept but `IpAddr` does
    // not parse, so a name whose last label is numeric is never a hostname.
    let tld = labels.last().copied().unwrap_or_default();
    if tld.bytes().all(|b| b.is_ascii_digit()) || tld.starts_with("0x") {
        return Err("Enter a valid host name.");
    }
    if !allow_private && (labels.len() == 1 || INTERNAL_SUFFIXES.iter().any(|s| host.ends_with(s)))
    {
        return Err("This host name is not allowed (internal network).");
    }
    Ok(host)
}

/// Resolves `host` and refuses it when any address is internal. For connection tests, where the
/// server is about to dial out. (A name can still change between this check and the connect;
/// that window is accepted and documented.)
pub async fn check_resolved(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<(), &'static str> {
    check_host(host, allow_private)?;
    if allow_private {
        return Ok(());
    }
    let addrs = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "The host name did not resolve in time.")?
    .map_err(|_| "The host name could not be resolved.")?;
    let mut any = false;
    for addr in addrs {
        any = true;
        if is_blocked_ip(addr.ip()) {
            return Err("This host resolves to an internal address.");
        }
    }
    if any {
        Ok(())
    } else {
        Err("The host name could not be resolved.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn internal_addresses_are_blocked() {
        for a in [
            "127.0.0.1",
            "127.255.0.9",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(is_blocked_ip(ip(a)), "{a} must be blocked");
        }
    }

    #[test]
    fn public_addresses_pass() {
        for a in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "100.63.255.255",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(!is_blocked_ip(ip(a)), "{a} must pass");
        }
    }

    #[test]
    fn hosts_are_normalised_and_policed() {
        assert_eq!(
            check_host(" SMTP.Example.com. ", false).unwrap(),
            "smtp.example.com"
        );
        assert_eq!(check_host("8.8.8.8", false).unwrap(), "8.8.8.8");
        for bad in [
            "",
            "localhost",
            "LOCALHOST.",
            "foo.localhost",
            "printer.local",
            "db.internal",
            "mailserver",
            "127.0.0.1",
            "[::1]",
            "::1",
            "10.0.0.5",
            "127.1",
            "2130706433",
            "0x7f.0.0.1",
            "a..b.com",
            "-a.example.com",
            "exa mple.com",
            "ex_ample.com",
            "evil.com/path",
            "evil.com:25",
            "user@evil.com",
        ] {
            assert!(check_host(bad, false).is_err(), "{bad:?} must be rejected");
        }
        assert!(check_host(&format!("{}.com", "a".repeat(64)), false).is_err());
    }

    #[test]
    fn the_allow_flag_lifts_only_the_address_rule() {
        for ok in ["localhost", "127.0.0.1", "::1", "mailserver", "10.0.0.5"] {
            assert!(check_host(ok, true).is_ok(), "{ok}");
        }
        for bad in ["", "a..b", "evil.com/x", "2130706433", "user@host"] {
            assert!(check_host(bad, true).is_err(), "{bad:?} stays invalid");
        }
    }

    #[tokio::test]
    async fn resolution_rejects_internal_targets() {
        assert!(check_resolved("127.0.0.1", 25, false).await.is_err());
        assert!(check_resolved("localhost", 25, false).await.is_err());
        assert!(check_resolved("127.0.0.1", 25, true).await.is_ok());
        assert!(check_resolved("8.8.8.8", 25, false).await.is_ok());
    }

    proptest! {
        #[test]
        fn any_private_v4_is_blocked(a in 0u8..=255, b in 0u8..=255, c in 0u8..=255) {
            prop_assert!(is_blocked_ip(IpAddr::from([10, a, b, c])));
            prop_assert!(is_blocked_ip(IpAddr::from([127, a, b, c])));
            prop_assert!(is_blocked_ip(IpAddr::from([192, 168, b, c])));
            prop_assert!(is_blocked_ip(IpAddr::from([169, 254, b, c])));
            prop_assert!(is_blocked_ip(IpAddr::from([172, 16 + (a % 16), b, c])));
        }

        #[test]
        fn mapped_v6_follows_the_v4_verdict(a in 0u8..=255, b in 0u8..=255, c in 0u8..=255, d in 0u8..=255) {
            let v4 = IpAddr::from([a, b, c, d]);
            let mapped = IpAddr::V6(std::net::Ipv4Addr::new(a, b, c, d).to_ipv6_mapped());
            prop_assert_eq!(is_blocked_ip(v4), is_blocked_ip(mapped));
        }

        #[test]
        fn check_host_never_panics(s in ".{0,300}") {
            let _ = check_host(&s, false);
            let _ = check_host(&s, true);
        }
    }
}
