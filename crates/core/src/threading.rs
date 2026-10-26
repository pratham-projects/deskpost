//! Ticket subject and Message-ID / In-Reply-To / References helpers.

use std::sync::LazyLock;

use regex::Regex;

/// Most ids kept from a `References` header or built for an outgoing one.
pub const MAX_REFERENCES: usize = 20;
const MAX_ID_LEN: usize = 255;

static TICKET_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[\s*ticket\s*#\s*(\d{1,9})\s*\]").unwrap());
static REPLY_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(re|fwd?|aw|wg|sv|vs|tr)\s*(\[\d+\])?\s*:\s*").unwrap());

fn strip_ctl(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// `"{project} Support - [Ticket #N]"`, with `Re: ` for replies. Control characters in the
/// project name (CR/LF would split the header) are dropped.
pub fn ticket_subject(project_name: &str, ticket_number: u32, is_reply: bool) -> String {
    let base = format!(
        "{} Support - [Ticket #{ticket_number}]",
        strip_ctl(project_name).trim()
    );
    if is_reply {
        format!("Re: {base}")
    } else {
        base
    }
}

/// Ticket number from a `[Ticket #N]` tag, tolerating `Re:`/`Fwd:` prefixes and any case.
pub fn parse_ticket_number(subject: &str) -> Option<u32> {
    TICKET_TAG.captures(subject)?[1].parse().ok()
}

/// Removes any run of `Re:`, `Fwd:`, `AW:`, ... prefixes.
pub fn strip_reply_prefixes(subject: &str) -> &str {
    let mut s = subject;
    while let Some(m) = REPLY_PREFIX.find(s) {
        s = &s[m.end()..];
    }
    s.trim()
}

fn is_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.split('.')
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// `<ticket-{id}@{domain}>`: the Message-ID of the first mail of a ticket.
/// `None` if the id is not a UUID or the domain is not a plain hostname.
pub fn ticket_message_id(ticket_id: &str, domain: &str) -> Option<String> {
    (is_uuid(ticket_id) && is_domain(domain)).then(|| format!("<ticket-{ticket_id}@{domain}>"))
}

/// `<ticket-{id}-reply-{message_id}@{domain}>`: Message-ID of an agent reply.
pub fn reply_message_id(ticket_id: &str, message_id: &str, domain: &str) -> Option<String> {
    (is_uuid(ticket_id) && is_uuid(message_id) && is_domain(domain))
        .then(|| format!("<ticket-{ticket_id}-reply-{message_id}@{domain}>"))
}

/// Ticket id out of a Message-ID we minted (either form). Foreign ids give `None`.
pub fn ticket_id_from_message_id(message_id: &str) -> Option<&str> {
    let inner = message_id.trim().strip_prefix('<')?.strip_suffix('>')?;
    let (local, domain) = inner.split_once('@')?;
    if !is_domain(domain) {
        return None;
    }
    let rest = local.strip_prefix("ticket-")?;
    let id = rest.get(..36)?;
    let tail = &rest[36..];
    let tail_ok = tail.is_empty() || tail.strip_prefix("-reply-").is_some_and(is_uuid);
    (is_uuid(id) && tail_ok).then_some(id)
}

/// All `<id>` tokens of an In-Reply-To / References header, in order, deduplicated and bounded.
/// Oversized or whitespace/control-containing ids are skipped.
pub fn parse_message_ids(header: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = header;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        let Some(end) = after.find('>') else { break };
        let tok = &after[..=end];
        // a '<' inside means the real token starts later (e.g. "<<a@b>")
        if let Some(inner_lt) = tok[1..].rfind('<') {
            rest = &after[inner_lt + 1..];
            continue;
        }
        rest = &after[end + 1..];
        let inner = &tok[1..tok.len() - 1];
        let ok = tok.len() <= MAX_ID_LEN
            && inner.contains('@')
            && !inner.chars().any(|c| c.is_whitespace() || c.is_control());
        if ok && !out.iter().any(|e| e == tok) {
            out.push(tok.to_string());
            if out.len() >= 1000 {
                break;
            }
        }
    }
    out
}

/// `References` for a reply to `parent_id`: previous chain plus the parent, deduplicated.
/// Capped at [`MAX_REFERENCES`], keeping the thread root and the newest entries.
pub fn build_references(previous: &[String], parent_id: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for id in previous.iter().map(String::as_str) {
        if !id.is_empty() && id != parent_id && !all.iter().any(|e| e == id) {
            all.push(id.to_string());
        }
    }
    if !parent_id.is_empty() {
        all.push(parent_id.to_string());
    }
    if all.len() > MAX_REFERENCES {
        let tail = all.split_off(all.len() - (MAX_REFERENCES - 1));
        all.truncate(1);
        all.extend(tail);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const T: &str = "0190a6f0-7c3e-7b52-9a1d-3c4f5e6a7b8c";
    const M: &str = "0190a6f1-0000-7000-8000-000000000001";

    #[test]
    fn subject_format() {
        assert_eq!(
            ticket_subject("Acme", 42, false),
            "Acme Support - [Ticket #42]"
        );
        assert_eq!(
            ticket_subject("Acme", 42, true),
            "Re: Acme Support - [Ticket #42]"
        );
    }

    #[test]
    fn subject_cannot_inject_headers() {
        let s = ticket_subject("Acme\r\nBcc: evil@x.com", 1, false);
        assert!(!s.contains('\r') && !s.contains('\n'));
    }

    #[test]
    fn ticket_number_round_trips_through_subject() {
        for reply in [false, true] {
            assert_eq!(
                parse_ticket_number(&ticket_subject("Acme", 42, reply)),
                Some(42)
            );
        }
    }

    #[test]
    fn ticket_number_parsing_is_tolerant_but_strict_on_digits() {
        assert_eq!(parse_ticket_number("RE: Fwd: x - [ticket #7]"), Some(7));
        assert_eq!(parse_ticket_number("[Ticket # 12 ]"), Some(12));
        assert_eq!(parse_ticket_number("Ticket #5"), None);
        assert_eq!(parse_ticket_number("[Ticket #]"), None);
        assert_eq!(parse_ticket_number("[Ticket #99999999999]"), None);
        assert_eq!(parse_ticket_number("[Ticket #-3]"), None);
        assert_eq!(parse_ticket_number(""), None);
    }

    #[test]
    fn reply_prefixes_are_stripped() {
        assert_eq!(strip_reply_prefixes("Re: RE: Fwd: Hello"), "Hello");
        assert_eq!(strip_reply_prefixes("AW: Frage"), "Frage");
        assert_eq!(strip_reply_prefixes("Re[2]: x"), "x");
        assert_eq!(strip_reply_prefixes("Reply to boss"), "Reply to boss");
        assert_eq!(strip_reply_prefixes("Regarding: x"), "Regarding: x");
    }

    #[test]
    fn message_ids_round_trip() {
        let first = ticket_message_id(T, "mail.example.com").unwrap();
        assert_eq!(first, format!("<ticket-{T}@mail.example.com>"));
        assert_eq!(ticket_id_from_message_id(&first), Some(T));
        let reply = reply_message_id(T, M, "mail.example.com").unwrap();
        assert_eq!(ticket_id_from_message_id(&reply), Some(T));
    }

    #[test]
    fn message_id_rejects_bad_parts() {
        assert_eq!(ticket_message_id("not-a-uuid", "a.com"), None);
        assert_eq!(ticket_message_id(T, "a.com>\r\nX: y"), None);
        assert_eq!(ticket_message_id(T, ""), None);
        assert_eq!(reply_message_id(T, "pending-1", "a.com"), None);
    }

    #[test]
    fn foreign_message_ids_are_not_ours() {
        for id in [
            "<abc@mail.gmail.com>",
            "<ticket-xyz@a.com>",
            &format!("<ticket-{T}-evil@a.com>"),
            &format!("<ticket-{T}-reply-nope@a.com>"),
            &format!("ticket-{T}@a.com"),
            &format!("<ticket-{T}@>"),
            "",
        ] {
            assert_eq!(ticket_id_from_message_id(id), None, "{id}");
        }
    }

    #[test]
    fn parse_ids_basic_and_dedup() {
        let h = "<a@x.com> <b@x.com>\r\n\t<a@x.com>";
        assert_eq!(parse_message_ids(h), vec!["<a@x.com>", "<b@x.com>"]);
        assert_eq!(parse_message_ids("<only@x.com>"), vec!["<only@x.com>"]);
        assert!(parse_message_ids("").is_empty());
    }

    #[test]
    fn parse_ids_skips_junk() {
        assert!(parse_message_ids("<no-at-sign>").is_empty());
        assert!(parse_message_ids("<a b@x.com>").is_empty());
        assert!(parse_message_ids("<unterminated@x.com").is_empty());
        assert_eq!(parse_message_ids("<<a@x.com>"), vec!["<a@x.com>"]);
        let long = format!("<{}@x.com>", "a".repeat(300));
        assert!(parse_message_ids(&long).is_empty());
    }

    #[test]
    fn references_append_dedupe_and_cap() {
        let prev = vec!["<a@x>".to_string(), "<b@x>".to_string()];
        assert_eq!(
            build_references(&prev, "<c@x>"),
            vec!["<a@x>", "<b@x>", "<c@x>"]
        );
        // the parent always ends the chain, even if it was already listed
        assert_eq!(build_references(&prev, "<a@x>"), vec!["<b@x>", "<a@x>"]);

        let many: Vec<String> = (0..50).map(|i| format!("<{i}@x>")).collect();
        let out = build_references(&many, "<new@x>");
        assert_eq!(out.len(), MAX_REFERENCES);
        assert_eq!(out.first().unwrap(), "<0@x>");
        assert_eq!(out.last().unwrap(), "<new@x>");
    }

    proptest! {
        #[test]
        fn subject_number_round_trip(n in 0u32..=999_999_999, name in "\\PC{0,30}", reply in any::<bool>()) {
            prop_assert_eq!(parse_ticket_number(&ticket_subject(&name, n, reply)), Some(n));
        }

        #[test]
        fn subject_never_has_control_chars(name in ".{0,40}", n in any::<u32>()) {
            prop_assert!(!ticket_subject(&name, n, true).chars().any(char::is_control));
        }

        #[test]
        fn message_id_round_trip(a in any::<u128>(), b in any::<u128>()) {
            let (t, m) = (uuid_like(a), uuid_like(b));
            let id = reply_message_id(&t, &m, "h.example.org").unwrap();
            prop_assert_eq!(ticket_id_from_message_id(&id), Some(t.as_str()));
        }

        #[test]
        fn parsers_never_panic(s in "\\PC*") {
            let _ = parse_message_ids(&s);
            let _ = parse_ticket_number(&s);
            let _ = strip_reply_prefixes(&s);
            let _ = ticket_id_from_message_id(&s);
        }

        #[test]
        fn parsed_ids_are_bracketed_unique_and_bounded(s in "[<>a-z@ .\\n]{0,200}") {
            let ids = parse_message_ids(&s);
            for (i, id) in ids.iter().enumerate() {
                prop_assert!(id.starts_with('<') && id.ends_with('>') && id.contains('@'));
                prop_assert!(!ids[..i].contains(id));
            }
        }

        #[test]
        fn references_bounded_and_end_with_parent(prev in proptest::collection::vec("<[a-z]{1,5}@x>", 0..60), parent in "<p[a-z]{0,3}@x>") {
            let out = build_references(&prev, &parent);
            prop_assert!(out.len() <= MAX_REFERENCES);
            prop_assert_eq!(out.last(), Some(&parent));
        }
    }

    fn uuid_like(n: u128) -> String {
        let h = format!("{n:032x}");
        format!(
            "{}-{}-{}-{}-{}",
            &h[..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
    }
}
