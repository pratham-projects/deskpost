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
    /// SMTP/IMAP connection tests: each one makes the server dial out.
    MailProbe,
    /// Password changes by a signed-in user (guessing the current password).
    PasswordChange,
    /// Reset-link requests: each one can send mail.
    ForgotPassword,
    /// Reset-token submissions (guessing).
    ResetPassword,
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
            Self::MailProbe => (5, Duration::from_secs(60)),
            Self::PasswordChange => (6, Duration::from_secs(60)),
            Self::ForgotPassword => (10, Duration::from_secs(3600)),
            Self::ResetPassword => (10, Duration::from_secs(60)),
        }
    }
}

const ACCOUNT_FAILURES: usize = 20;
const ACCOUNT_WINDOW: Duration = Duration::from_secs(600);
/// Reset mails per address per window, whether or not the account exists.
const RESET_MAILS: usize = 3;
const RESET_WINDOW: Duration = Duration::from_secs(3600);

pub struct RateLimiter {
    max_keys: usize,
    hits: Mutex<HashMap<(Rule, IpAddr), VecDeque<Instant>>>,
    /// Failed sign-ins per existing account, in their own table so address buckets cannot evict them.
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Reset requests per address (own table, same reason).
    resets: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(max_keys: usize) -> Self {
        Self {
            max_keys: max_keys.max(1),
            hits: Mutex::default(),
            failures: Mutex::default(),
            resets: Mutex::default(),
        }
    }

    pub fn check(&self, rule: Rule, ip: IpAddr) -> Result<(), ApiError> {
        self.check_at(rule, ip, Instant::now())
            .map_err(|retry| ApiError::RateLimited {
                retry_after_secs: retry.as_secs_f64().ceil().max(1.0) as u64,
            })
    }

    /// Reject sign-in for an account that already failed too often, whoever asks. Only wrong
    /// passwords count (see [`Self::account_failed`]) and only for accounts that exist.
    pub fn account_check(&self, name: &str) -> Result<(), ApiError> {
        let now = Instant::now();
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let Some(q) = f.get_mut(&name.to_lowercase()) else {
            return Ok(());
        };
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= ACCOUNT_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= ACCOUNT_FAILURES {
            let wait = ACCOUNT_WINDOW - now.duration_since(*q.front().expect("not empty"));
            return Err(ApiError::RateLimited {
                retry_after_secs: wait.as_secs_f64().ceil().max(1.0) as u64,
            });
        }
        Ok(())
    }

    /// May a reset mail go to `email` now? Every request counts, known address or not, so the
    /// answer says nothing about the account; callers drop the mail silently on `false`. A full
    /// table of live entries refuses new addresses (fail closed) instead of evicting counters.
    pub fn reset_mail_allowed(&self, email: &str) -> bool {
        let now = Instant::now();
        let mut r = self.resets.lock().unwrap_or_else(|e| e.into_inner());
        let key = email.to_lowercase();
        if !r.contains_key(&key) && r.len() >= self.max_keys {
            r.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < RESET_WINDOW)
            });
            if r.len() >= self.max_keys {
                return false;
            }
        }
        let q = r.entry(key).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= RESET_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= RESET_MAILS {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Count a wrong password against an existing account.
    pub fn account_failed(&self, name: &str) {
        let now = Instant::now();
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if f.len() >= self.max_keys {
            f.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.duration_since(*t) < ACCOUNT_WINDOW)
            });
        }
        // Still full of live entries: keep counting the ones we have rather than evicting them.
        let key = name.to_lowercase();
        if f.len() < self.max_keys || f.contains_key(&key) {
            let q = f.entry(key).or_default();
            while q
                .front()
                .is_some_and(|t| now.duration_since(*t) >= ACCOUNT_WINDOW)
            {
                q.pop_front();
            }
            q.push_back(now);
        }
    }

    /// A correct password resets the count.
    pub fn account_ok(&self, name: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&name.to_lowercase());
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
