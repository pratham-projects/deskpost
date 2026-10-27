//! Closed value sets stored as TEXT with CHECK constraints.

macro_rules! text_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
        #[sqlx(type_name = "text", rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s { $($s => Ok(Self::$variant),)+ _ => Err(format!("invalid {}: {s}", stringify!($name))) }
            }
        }
    };
}

text_enum!(
    /// Global role (`users.role`) and project role (`project_members.role`).
    Role { Admin => "admin", Agent => "agent" }
);
text_enum!(
    /// `users.status` and `projects.status`.
    Status { Active => "active", Inactive => "inactive", Suspended => "suspended" }
);
text_enum!(TicketStatus {
    Pending => "pending", InProgress => "in_progress", OnHold => "on_hold",
    Resolved => "resolved", Closed => "closed",
});
text_enum!(Priority { Low => "low", Normal => "normal", High => "high", Urgent => "urgent" });
text_enum!(TicketSource { Web => "web", Admin => "admin", Email => "email" });
text_enum!(SenderType { Agent => "agent", Client => "client" });
text_enum!(MessageType { Reply => "reply", Note => "note", System => "system" });
text_enum!(EmailResponseStatus { Created => "created", Resolved => "resolved" });
text_enum!(EmailLogStatus {
    Pending => "pending", Sent => "sent", Failed => "failed", Bounced => "bounced",
});
