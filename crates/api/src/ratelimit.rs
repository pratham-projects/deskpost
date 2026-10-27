//! In-process sliding-window rate limiter, keyed by client address. Memory is bounded: each key
//! keeps at most `limit` timestamps and the table holds at most `max_keys` keys (expired keys go
//! first, then the least recently seen one).

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    Login,
    Register,
    Refresh,
    /// Unauthenticated widget endpoints.
    Public,
    Webhook,
}

impl Rule {
    /// `(requests, window)` per client.
    fn quota(self) -> (usize, Duration) {
        match self {
            Self::Login => (10, Duration::from_secs(60)),
            Self::Register => (5, Duration::from_secs(3600)),
            Self::Refresh => (30, Duration::from_secs(60)),
            Self::Public => (20, Duration::from_secs(60)),
            Self::Webhook => (120, Duration::from_secs(60)),
        }
    }
}

pub struct RateLimiter {
    max_keys: usize,
    hits: Mutex<HashMap<(Rule, IpAddr), VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(max_keys: usize) -> Self {
        Self {
            max_keys: max_keys.max(1),
            hits: Mutex::default(),
        }
    }

    pub fn check(&self, rule: Rule, ip: IpAddr) -> Result<(), ApiError> {
        self.check_at(rule, ip, Instant::now())
            .map_err(|retry| ApiError::RateLimited {
                retry_after_secs: retry.as_secs_f64().ceil().max(1.0) as u64,
            })
    }

    /// `Err(wait)` when the client is over quota. Rejected requests are not recorded.
    pub fn check_at(&self, rule: Rule, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let (limit, window) = rule.quota();
        let key = (rule, bucket(ip));
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        if !hits.contains_key(&key) && hits.len() >= self.max_keys {
            hits.retain(|(r, _), q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < r.quota().1)
            });
            if hits.len() >= self.max_keys {
                let oldest = hits
                    .iter()
                    .min_by_key(|(_, q)| q.back().copied())
                    .map(|(k, _)| *k);
                if let Some(k) = oldest {
                    hits.remove(&k);
                }
            }
        }
        let q = hits.entry(key).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= limit {
            let oldest = *q.front().expect("limit >= 1");
            return Err(window - now.duration_since(oldest));
        }
        q.push_back(now);
        Ok(())
    }

    pub fn tracked_keys(&self) -> usize {
        self.hits.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// A whole IPv6 /64 is one customer; counting single addresses would let one host mint 2^64 keys.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let mut o = v6.octets();
            o[8..].fill(0);
            IpAddr::from(o)
        }
        v4 => v4,
    }
}
