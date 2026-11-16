//! SSRF guard shared by every outbound connection (IMAP and SMTP). The host is resolved here, each
//! address is checked, and the connection goes to the checked `SocketAddr` itself (the hostname is
//! kept only for TLS), so a DNS answer cannot change between check and connect.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::Error;

/// Reserved IPv4 ranges not covered by the std `is_*` helpers: (network, prefix length).
const BLOCKED_V4: [([u8; 4], u8); 9] = [
    ([0, 0, 0, 0], 8),
    ([100, 64, 0, 0], 10),  // carrier-grade NAT
    ([192, 0, 0, 0], 24),   // IETF protocol assignments
    ([192, 0, 2, 0], 24),   // documentation
    ([192, 88, 99, 0], 24), // deprecated 6to4 relay anycast
    ([198, 18, 0, 0], 15),  // benchmarking
    ([198, 51, 100, 0], 24),
    ([203, 0, 113, 0], 24),
    ([240, 0, 0, 0], 4), // reserved + broadcast
];

fn v4_in(o: [u8; 4], net: [u8; 4], prefix: u8) -> bool {
    let (a, n) = (u32::from_be_bytes(o), u32::from_be_bytes(net));
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    a & mask == n & mask
}

/// True for loopback, private, link-local, multicast, reserved and tunnel-embedded internal addresses.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || BLOCKED_V4
                    .iter()
                    .any(|&(net, p)| v4_in(v4.octets(), net, p))
        }
        IpAddr::V6(v6) => {
            let o = v6.octets();
            let s = v6.segments();
            let embedded = |a: usize| IpAddr::from([o[a], o[a + 1], o[a + 2], o[a + 3]]);
            // NAT64 (64:ff9b::/96) carries an IPv4 address in the last 32 bits.
            if o[..12] == [0, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
                return is_blocked_ip(embedded(12));
            }
            // 6to4 (2002::/16) carries one right after the prefix.
            if s[0] == 0x2002 {
                return is_blocked_ip(embedded(2));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] & 0xffc0) == 0xfec0 // deprecated site local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || (s[0] == 0x2001 && s[1] == 0) // Teredo
                || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 1) // local-use NAT64
        }
    }
}

/// Resolve `host:port` and return an address that is safe to dial. With `allow_private` every
/// address passes (local test servers); otherwise a single blocked address in the answer refuses
/// the host, so a mixed public/private record cannot be used to pick the private one.
pub async fn resolve(host: &str, port: u16, allow_private: bool) -> Result<SocketAddr, Error> {
    let blocked = |reason: &str| Error::Invalid {
        field: "mail host",
        reason: reason.to_string(),
    };
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host.trim_end_matches('.'), port)),
    )
    .await
    .map_err(|_| Error::Timeout("dns lookup"))??
    .collect();
    if !allow_private && addrs.iter().any(|a| is_blocked_ip(a.ip())) {
        return Err(blocked("resolves to an internal address"));
    }
    addrs
        .into_iter()
        .next()
        .ok_or_else(|| blocked("could not be resolved"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn tunnel_and_reserved_ranges_are_blocked() {
        for a in [
            "2002:7f00:1::1",                       // 6to4 of 127.0.0.1
            "2002:a9fe:a9fe::1",                    // 6to4 of 169.254.169.254
            "2001:0:4136:e378:8000:63bf:3fff:fdd2", // Teredo
            "64:ff9b:1::1",
            "192.88.99.1",
            "fec0::1",
            "::",
        ] {
            assert!(is_blocked_ip(ip(a)), "{a}");
        }
        assert!(!is_blocked_ip(ip("2002:0808:0808::1")), "6to4 of 8.8.8.8");
        assert!(!is_blocked_ip(ip("8.8.8.8")));
    }

    #[tokio::test]
    async fn resolve_refuses_internal_unless_allowed() {
        assert!(resolve("127.0.0.1", 25, false).await.is_err());
        assert!(resolve("localhost", 25, false).await.is_err());
        assert_eq!(resolve("127.0.0.1", 25, true).await.unwrap().port(), 25);
        assert!(resolve("8.8.8.8", 25, false).await.is_ok());
    }
}
