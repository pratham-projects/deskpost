//! Client address behind reverse proxies. `X-Forwarded-For` is honoured only when the TCP peer is
//! in `SD_TRUSTED_PROXIES`, and then read from the right: the first address that is not itself a
//! trusted proxy is the client. Everything to its left is attacker-controlled and ignored.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header::HeaderName;
use axum::http::request::Parts;
use ipnet::IpNet;

use crate::error::ApiError;
use crate::state::AppState;

pub fn resolve(peer: IpAddr, forwarded_for: &str, trusted: &[IpNet]) -> IpAddr {
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|n| n.contains(ip));
    let peer = peer.to_canonical();
    if !is_trusted(&peer) {
        return peer;
    }
    let mut client = None;
    for entry in forwarded_for.rsplit(',') {
        let Ok(ip) = entry.trim().parse::<IpAddr>() else {
            // Garbage from our own proxy chain: do not guess.
            return client.unwrap_or(peer);
        };
        let ip = ip.to_canonical();
        client = Some(ip);
        if !is_trusted(&ip) {
            break;
        }
    }
    client.unwrap_or(peer)
}

/// The resolved client address. Without a socket (in-process tests) it is `0.0.0.0` unless the
/// test inserts `ConnectInfo<SocketAddr>` into the request extensions.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |c| c.0.ip());
        let forwarded = parts
            .headers
            .get_all(HeaderName::from_static("x-forwarded-for"))
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        Ok(Self(resolve(
            peer,
            &forwarded,
            &state.config.trusted_proxies,
        )))
    }
}
