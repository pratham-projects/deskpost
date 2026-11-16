//! Wire shapes shared by several route modules (`docs/API.md` section 2).

use chrono::{DateTime, SecondsFormat, Utc};
use dp_db::users::User;
use serde::Serialize;

/// RFC 3339 UTC with second precision: `2026-10-27T10:00:00Z`.
pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: dp_db::Id,
    pub email: String,
    pub name: String,
    pub role: &'static str,
    pub status: &'static str,
    pub phone: Option<String>,
    pub timezone: String,
    pub avatar_url: Option<String>,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&User> for UserDto {
    fn from(u: &User) -> Self {
        Self {
            id: u.id,
            email: u.email.clone(),
            name: u.name.clone(),
            role: u.role.as_str(),
            status: u.status.as_str(),
            phone: u.phone.clone(),
            timezone: u.timezone.clone(),
            avatar_url: u.avatar_url.clone(),
            last_login_at: u.last_login_at.map(ts),
            created_at: ts(u.created_at),
            updated_at: ts(u.updated_at),
        }
    }
}
