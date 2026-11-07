//! Environment configuration. Every problem is collected so one startup attempt reports them all.

use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;

use ipnet::IpNet;

const MIN_SECRET_BYTES: usize = 32;
const MAX_LIMIT_BYTES: usize = 512 * 1024 * 1024;

/// Substrings (lowercase) that mark a copy-pasted example value rather than a generated secret.
const PLACEHOLDERS: &[&str] = &[
    "changeme",
    "change-me",
    "change_me",
    "replace",
    "your-",
    "your_",
    "placeholder",
    "example",
    "insecure",
    "dummy",
    "default",
    "password",
    "secret",
    "todo",
];

#[derive(Debug, Clone, Copy)]
pub struct Argon2Cost {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Argon2Cost {
    /// OWASP minimum for Argon2id (19 MiB, t=2, p=1).
    pub const PRODUCTION: Self = Self {
        memory_kib: 19 * 1024,
        iterations: 2,
        parallelism: 1,
    };
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub api_version: String,
    pub production: bool,
    pub jwt_secret: Vec<u8>,
    pub secret_key: Vec<u8>,
    pub trusted_proxies: Vec<IpNet>,
    pub cookie_secure: bool,
    pub cors_origins: Vec<String>,
    pub storage_dir: PathBuf,
    pub body_limit: usize,
    pub upload_limit: usize,
    pub webhook_limit: usize,
    pub allow_signup: bool,
    /// With sign-up open, the first account becomes the global admin (`SD_FIRST_USER_ADMIN`).
    pub first_user_admin: bool,
    /// `SD_ALLOW_PRIVATE_MAIL_HOSTS`: let SMTP/IMAP settings point at loopback/private addresses.
    pub allow_private_mail_hosts: bool,
    pub argon2: Argon2Cost,
    pub mail: crate::mail_config::MailConfig,
}

/// Secrets and the database URL (which carries a password) stay out of logs.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("port", &self.port)
            .field("api_version", &self.api_version)
            .field("production", &self.production)
            .field("cors_origins", &self.cors_origins)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ConfigError(pub Vec<String>);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid configuration: {}", self.0.join("; "))
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut errs = Vec::new();
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        let database_url = get("DATABASE_URL").unwrap_or_else(|| {
            errs.push("DATABASE_URL is required".into());
            String::new()
        });
        let jwt_secret = secret(&get, "JWT_SECRET", &mut errs);
        let secret_key = secret(&get, "SD_SECRET_KEY", &mut errs);
        if !jwt_secret.is_empty() && jwt_secret == secret_key {
            errs.push("SD_SECRET_KEY must differ from JWT_SECRET".into());
        }

        let production = match get("SD_ENV").as_deref() {
            None | Some("production") => true,
            Some("development") => false,
            Some(_) => {
                errs.push("SD_ENV must be `production` or `development`".into());
                true
            }
        };
        let api_version = get("API_VERSION").unwrap_or_else(|| "v3".into());
        // Goes into route and cookie paths, so keep it to a harmless alphabet.
        if api_version.len() > 16
            || !api_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        {
            errs.push("API_VERSION must be 1-16 characters of [A-Za-z0-9._-]".into());
        }

        let trusted_proxies = get("SD_TRUSTED_PROXIES")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| {
                        parse_cidr(s).or_else(|| {
                            errs.push(format!("SD_TRUSTED_PROXIES: `{s}` is not an IP or CIDR"));
                            None
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let cors_origins = get("FRONTEND_URL")
            .map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| {
                        parse_origin(s).or_else(|| {
                            errs.push(format!(
                                "FRONTEND_URL: `{s}` must be an origin like https://app.example.com"
                            ));
                            None
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let config = Self {
            database_url,
            port: num(&get, "PORT", 3000, 1, u16::MAX as usize, &mut errs) as u16,
            api_version,
            production,
            jwt_secret,
            secret_key,
            trusted_proxies,
            cookie_secure: flag(&get, "COOKIE_SECURE", production, &mut errs),
            cors_origins,
            storage_dir: get("STORAGE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| "./data/storage".into()),
            body_limit: num(
                &get,
                "SD_BODY_LIMIT_BYTES",
                64 * 1024,
                1,
                MAX_LIMIT_BYTES,
                &mut errs,
            ),
            upload_limit: num(
                &get,
                "SD_UPLOAD_LIMIT_BYTES",
                26 * 1024 * 1024,
                1,
                MAX_LIMIT_BYTES,
                &mut errs,
            ),
            webhook_limit: num(
                &get,
                "SD_WEBHOOK_LIMIT_BYTES",
                36 * 1024 * 1024,
                1,
                MAX_LIMIT_BYTES,
                &mut errs,
            ),
            allow_signup: flag(&get, "SD_ALLOW_SIGNUP", false, &mut errs),
            first_user_admin: flag(&get, "SD_FIRST_USER_ADMIN", false, &mut errs),
            allow_private_mail_hosts: flag(&get, "SD_ALLOW_PRIVATE_MAIL_HOSTS", false, &mut errs),
            argon2: Argon2Cost::PRODUCTION,
            mail: crate::mail_config::MailConfig::from_lookup(&get, &mut errs),
        };
        let m = &config.mail;
        if m.max_file_bytes > m.max_request_bytes {
            errs.push("SD_MAX_FILE_BYTES must not exceed SD_MAX_REQUEST_BYTES".into());
        }
        if config.upload_limit <= m.max_request_bytes {
            errs.push(
                "SD_UPLOAD_LIMIT_BYTES must be larger than SD_MAX_REQUEST_BYTES (multipart overhead)"
                    .into(),
            );
        }
        if config.webhook_limit < m.max_request_bytes / 3 * 4 {
            errs.push(
                "SD_WEBHOOK_LIMIT_BYTES must fit SD_MAX_REQUEST_BYTES as base64 (4/3 of it)".into(),
            );
        }
        if errs.is_empty() {
            Ok(config)
        } else {
            Err(ConfigError(errs))
        }
    }
}

pub(crate) fn secret(
    get: &impl Fn(&str) -> Option<String>,
    name: &str,
    errs: &mut Vec<String>,
) -> Vec<u8> {
    let Some(v) = get(name) else {
        errs.push(format!("{name} is required"));
        return Vec::new();
    };
    let lower = v.to_ascii_lowercase();
    let mut distinct = v.bytes().collect::<Vec<_>>();
    distinct.sort_unstable();
    distinct.dedup();
    if v.len() < MIN_SECRET_BYTES {
        errs.push(format!("{name} must be at least {MIN_SECRET_BYTES} bytes"));
    } else if PLACEHOLDERS.iter().any(|p| lower.contains(p)) || distinct.len() < 8 {
        errs.push(format!(
            "{name} looks like a placeholder; generate one with `openssl rand -base64 48`"
        ));
    }
    v.into_bytes()
}

pub(crate) fn num(
    get: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: usize,
    min: usize,
    max: usize,
    errs: &mut Vec<String>,
) -> usize {
    match get(name).map(|v| v.parse::<usize>()) {
        None => default,
        Some(Ok(n)) if (min..=max).contains(&n) => n,
        Some(_) => {
            errs.push(format!("{name} must be an integer between {min} and {max}"));
            default
        }
    }
}

pub(crate) fn flag(
    get: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: bool,
    errs: &mut Vec<String>,
) -> bool {
    match get(name).as_deref() {
        None => default,
        Some("true" | "1") => true,
        Some("false" | "0") => false,
        Some(_) => {
            errs.push(format!("{name} must be true or false"));
            default
        }
    }
}

/// `10.0.0.0/8`, `::1/128` or a bare address (a single-host network).
fn parse_cidr(s: &str) -> Option<IpNet> {
    s.parse::<IpNet>()
        .ok()
        .or_else(|| s.parse::<IpAddr>().ok().map(IpNet::from))
}

/// `scheme://host[:port]` with no path, query, userinfo or wildcard. A trailing `/` is tolerated.
fn parse_origin(s: &str) -> Option<String> {
    let s = s.strip_suffix('/').unwrap_or(s).to_ascii_lowercase();
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let ok = !rest.is_empty()
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    ok.then_some(s)
}
