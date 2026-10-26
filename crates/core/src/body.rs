//! Inbound body handling: HTML to text and reply stripping (port of `emailBodyExtractor.ts`).
//!
//! `clean_body` keeps what the customer just wrote: text above and between quoted blocks, and
//! drops quoted replies (`>` lines, "On ... wrote:" in several languages), Outlook / forwarded
//! blocks, and signatures. Everything is linear time and bounded by [`MAX_INPUT`].
//!
//! Differences from the TypeScript product: a bare `From:` line is only a quote header when a
//! `Sent:`/`Date:` line follows (the TS version cut every message at the first `From:` line);
//! embedded leading headers are only stripped when at least two form a block; HTML
//! `<blockquote>` content is turned into `>` lines so HTML-only replies get stripped too.

use std::sync::LazyLock;

use regex::Regex;

/// Input beyond this many bytes is ignored (cut on a char boundary).
pub const MAX_INPUT: usize = 1 << 20;
const MAX_TAG_LEN: usize = 2048;
/// Total bytes all tag scans may read, as a multiple of the input length. Beyond it a `<` is text.
const TAG_SCAN_BUDGET: usize = 4;

fn clip(s: &str) -> &str {
    let mut end = MAX_INPUT.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------- html to text

const BLOCK_TAGS: [&str; 24] = [
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
    "pre",
    "hr",
    "section",
    "article",
    "header",
    "footer",
    "dl",
    "dt",
    "dd",
];
const SKIP_CONTENT: [&str; 4] = ["script", "style", "head", "title"];

fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() || from > h.len() - n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let num = name.strip_prefix('#')?;
            let cp = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            let c = char::from_u32(cp)?;
            (!c.is_control() || c == '\n' || c == '\t').then_some(c)
        }
    }
}

struct Writer {
    out: String,
    quote_depth: usize,
    pending_space: bool,
}

impl Writer {
    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    fn newline(&mut self) {
        self.pending_space = false;
        if !self.at_line_start() {
            self.out.push('\n');
        }
    }

    fn text(&mut self, s: &str) {
        for c in s.chars() {
            if c.is_whitespace() {
                self.pending_space = true;
                continue;
            }
            if self.at_line_start() && self.quote_depth > 0 {
                self.out.push_str("> ");
            } else if self.pending_space && !self.at_line_start() {
                self.out.push(' ');
            }
            self.pending_space = false;
            self.out.push(c);
        }
    }
}

/// Converts an HTML fragment/document to plain text. Scripts, styles, head, comments and tags
/// are dropped (no link targets, no image text), block tags become line breaks, common entities
/// are decoded, and `<blockquote>` content is prefixed with `> `.
pub fn html_to_text(html: &str) -> String {
    let html = clip(html);
    let mut w = Writer {
        out: String::new(),
        quote_depth: 0,
        pending_space: false,
    };
    let mut i = 0;
    let mut budget = html.len().saturating_mul(TAG_SCAN_BUDGET);
    while i < html.len() {
        let Some(rel) = html[i..].find(['<', '&']) else {
            w.text(&html[i..]);
            break;
        };
        w.text(&html[i..i + rel]);
        i += rel;
        let rest = &html[i..];
        if let Some(after) = rest.strip_prefix('&') {
            let decoded = after
                .find(';')
                .filter(|&e| e <= 10)
                .and_then(|e| decode_entity(&after[..e]).map(|c| (c, e + 1)));
            match decoded {
                Some((c, used)) => {
                    let mut buf = [0u8; 4];
                    w.text(c.encode_utf8(&mut buf));
                    i += 1 + used;
                }
                None => {
                    w.text("&");
                    i += 1;
                }
            }
            continue;
        }
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        let is_tag = rest[1..]
            .starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?');
        let tag_end = if is_tag && budget > 0 {
            let r = tag_end(rest);
            budget = budget.saturating_sub(r.map_or(rest.len().min(MAX_TAG_LEN), |e| e + 1));
            r
        } else {
            None
        };
        let Some(end) = tag_end else {
            w.text("<");
            i += 1;
            continue;
        };
        let inner = &rest[1..end];
        let closing = inner.starts_with('/');
        let name: String = inner
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        i += end + 1;
        if !closing && SKIP_CONTENT.contains(&name.as_str()) && !inner.ends_with('/') {
            i = find_ci(html, &format!("</{name}"), i).unwrap_or(html.len());
            continue;
        }
        if BLOCK_TAGS.contains(&name.as_str()) {
            if name == "blockquote" {
                w.quote_depth = if closing {
                    w.quote_depth.saturating_sub(1)
                } else {
                    w.quote_depth + 1
                };
            }
            w.newline();
        } else if matches!(name.as_str(), "td" | "th") {
            w.pending_space = true;
        }
    }
    tidy(&w.out)
}

/// Index of the closing `>` of the tag at the start of `s`, honouring quoted attribute values.
/// `None` if there is none within [`MAX_TAG_LEN`] (then the `<` is literal text).
fn tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &b) in s.as_bytes().iter().enumerate().take(MAX_TAG_LEN) {
        match (quote, b) {
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Trim trailing spaces per line and squeeze 3+ newlines to 2.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        let line = line.trim_end();
        if line.trim_start_matches(['>', ' ']).is_empty() && line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------- reply stripping

const WROTE: &str = r"(wrote|a\s+[eé]crit|escribi[oó]|schrieb[^:]{0,20}|escreveu|ha\s+scritto)";

static QUOTE_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^(on|le|el|am|em|il)\s+.{{8,300}}?\s*{WROTE}\s*:\s*$"
    ))
    .unwrap()
});
static QUOTE_HEADER_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(on|le|el|am|em|il)\s+\S").unwrap());
static FORWARD_OR_ORIGINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(-{3,}\s*(forwarded message|original message|original appointment)\s*-*|begin forwarded message:?)\s*$",
    )
    .unwrap()
});
static SIGNATURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^((--|---|—)|(sent from my|enviado desde mi|envoy[eé] de mon|gesendet von meinem)(\s+[\w\-]+){1,3}\s*\.?|(sent from mail for windows|sent from outlook|get outlook for |sent with slack|view in slack:).{0,80})$",
    )
    .unwrap()
});
static FROM_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^from:\s*\S").unwrap());
static SENT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(sent|date):\s*\S").unwrap());
static HEADER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(from|to|cc|bcc|subject|date|reply-to|mime-version|content-type):\s*\S")
        .unwrap()
});
static RULE_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^_{5,}$").unwrap());

/// Drops a leading block of 2+ header lines (a raw message pasted as the body).
fn strip_leading_headers(lines: &[&str]) -> usize {
    let first = lines
        .iter()
        .position(|l| !l.trim().is_empty())
        .unwrap_or(lines.len());
    let mut i = first;
    let mut headers = 0;
    while i < lines.len().min(first + 30) {
        let l = lines[i];
        if HEADER_LINE.is_match(l.trim()) {
            headers += 1;
        } else if !(l.starts_with([' ', '\t']) && headers > 0) {
            break;
        }
        i += 1;
    }
    if headers >= 2 && lines.get(i).is_none_or(|l| l.trim().is_empty()) {
        i
    } else {
        first
    }
}

/// The part of `text` the sender just wrote, without quoted replies, forwards and signatures.
pub fn clean_body(text: &str) -> String {
    let text = clip(text).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<&str> = Vec::new();
    let mut in_quote = false;
    let mut i = strip_leading_headers(&lines);

    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();

        if SIGNATURE.is_match(t) || FORWARD_OR_ORIGINAL.is_match(t) {
            break;
        }
        if FROM_LINE.is_match(t)
            && lines[i + 1..]
                .iter()
                .take(5)
                .any(|l| SENT_LINE.is_match(l.trim()))
        {
            if out.last().is_some_and(|l| RULE_LINE.is_match(l.trim())) {
                out.pop();
            }
            break;
        }
        if let Some(skip) = quote_header_len(&lines, i) {
            in_quote = true;
            i += skip;
            continue;
        }
        if line.trim_start().starts_with('>') {
            in_quote = true;
        } else if t.is_empty() {
            if !in_quote {
                out.push(line);
            }
        } else {
            in_quote = false;
            out.push(line);
        }
        i += 1;
    }
    out.join("\n").trim().to_string()
}

/// Number of lines taken by a quote header starting at `i` (clients wrap long ones over 1-3 lines).
fn quote_header_len(lines: &[&str], i: usize) -> Option<usize> {
    let first = lines[i].trim();
    if !QUOTE_HEADER_START.is_match(first) || first.len() > 300 {
        return None;
    }
    let mut joined = first.to_string();
    for n in 1..=3 {
        if QUOTE_HEADER.is_match(&joined) {
            return Some(n);
        }
        let next = lines.get(i + n)?.trim();
        if next.is_empty() || joined.len() + next.len() > 400 {
            return None;
        }
        joined.push(' ');
        joined.push_str(next);
    }
    QUOTE_HEADER.is_match(&joined).then_some(4)
}

/// Body for a ticket message: plain text part if it has content, else the HTML part converted;
/// quoted replies and signatures removed either way.
pub fn body_from_parts(text: Option<&str>, html: Option<&str>) -> String {
    match (text, html) {
        (Some(t), _) if !t.trim().is_empty() => clean_body(t),
        (_, Some(h)) if !h.trim().is_empty() => clean_body(&html_to_text(h)),
        _ => String::new(),
    }
}

/// An email with attachments but no text of its own.
pub fn is_attachment_only(body: &str, attachment_count: usize) -> bool {
    attachment_count > 0 && body.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Test emails are given as full raw text with headers; `clean_body` strips the header block.
    fn extract(raw: &str) -> String {
        clean_body(&raw.replace('\n', "\r\n"))
    }

    // ---- ported: emailBodyExtractor cases 1..20 ----

    #[test]
    fn case01_mixed_languages() {
        let r = extract(
            "From: marie@example.fr\nTo: support@company.com\nSubject: RE: Project Status\nMIME-Version: 1.0\nContent-Type: text/plain; charset=\"UTF-8\"\n\nYes, I can confirm the deadline. We are on track.\n\nLe 20 décembre 2025 à 10:30, Support <support@company.com> a écrit :\n> The project status is critical\n> Please confirm by end of day\n> Merci pour votre réponse rapide",
        );
        assert!(r.contains("Yes, I can confirm the deadline"));
        assert!(!r.contains("The project status is critical"));
        assert!(!r.contains("Merci pour"));
    }

    #[test]
    fn case02_forwarded_inside_reply() {
        let r = extract(
            "From: john@company.com\nTo: support@example.com\nSubject: RE: Bug Report - Urgent\n\nThis is critical! See below.\n\n---------- Forwarded message ---------\nFrom: client@customer.com\nDate: Wed, Dec 19, 2025 at 3:45 PM\nSubject: BUG: System crash on login\nTo: john@company.com\n\nThe system crashes every time I login. Very urgent!\n\nOn Wed, Dec 19, 2025 at 3:30 PM Support <support@customer.com> wrote:\n> We received your ticket #12345\n> Investigating...",
        );
        assert!(r.contains("This is critical!"));
        assert!(!r.contains("The system crashes"));
        assert!(!r.contains("We received your ticket"));
    }

    #[test]
    fn case03_plain_part_with_outlook_original() {
        let r = extract(
            "From: bob@outlook.com\nTo: support@example.com\nSubject: RE: Invoice #INV001\nContent-Type: text/plain; charset=\"UTF-8\"\n\nApproved! Please proceed with payment processing.\n\n-----Original Message-----\nFrom: Finance <finance@company.com>\nSent: Sunday, December 21, 2025 8:45 AM\nSubject: Invoice #INV001\n\nInvoice ready for approval.",
        );
        assert_eq!(r, "Approved! Please proceed with payment processing.");
    }

    #[test]
    fn case04_text_before_and_after_quote() {
        let r = extract(
            "From: dev@startup.com\nTo: support@example.com\nSubject: RE: API Implementation\n\nWait, let me reconsider. Actually the original request makes sense.\n\nOn Sat, Dec 20, 2025 at 11:22 PM Support <support@example.com> wrote:\n> Can you implement the OAuth flow?\n> We need it for the mobile app.\n\nYes, I'll start working on it tomorrow. Thanks for clarifying!",
        );
        assert!(r.contains("Wait, let me reconsider"));
        assert!(r.contains("I'll start working on it tomorrow"));
        assert!(!r.contains("Can you implement"));
    }

    #[test]
    fn case05_signature_with_dashes() {
        let r = extract(
            "From: consultant@firm.com\nTo: support@example.com\nSubject: RE: Consultation Services\n\nGreat! I can help with this project.\n\n---\nJohn Smith\nSenior Consultant\nStrategic Solutions Firm\n\nOffice: +1-555-0123\n---\n\nOn Dec 20, 2025, Support wrote:\n> Do you have bandwidth next month?",
        );
        assert!(r.contains("Great! I can help"));
        assert!(!r.contains("John Smith"));
        assert!(!r.contains("Do you have bandwidth"));
    }

    #[test]
    fn case06_reply_to_forwarded_inception() {
        let r = extract(
            "From: charlie@example.com\nTo: alice@example.com\nSubject: Re: Project Update\n\nLooks good! Let's proceed.\n\nOn Dec 20, 2025, Bob <bob@example.com> wrote:\n> ------Forwarded message-------\n> From: Alice <alice@example.com>\n> \n> I think we should update the design\n> \n> On Dec 19, Alice <alice@example.com> wrote:\n> > Current version is outdated",
        );
        assert_eq!(r, "Looks good! Let's proceed.");
    }

    #[test]
    fn case07_no_space_before_wrote() {
        let r = extract(
            "From: user@gmail.com\nTo: support@example.com\nSubject: RE: Support Ticket #5678\n\nThanks for your help! That solved it.\n\nOn Fri, Dec 19, 2025 at 2:15 PM Support <support@example.com>wrote:\n> Your issue has been resolved\n> Please confirm receipt",
        );
        assert_eq!(r, "Thanks for your help! That solved it.");
    }

    #[test]
    fn case08_mobile_signature() {
        let r = extract(
            "From: exec@company.com\nTo: support@example.com\nSubject: RE: Budget Approval\n\nApproved.\n\nOn Dec 20, 2025, Finance <finance@example.com> wrote:\n> Budget for Q1 2026 requires approval\n\nSent from my iPhone",
        );
        assert_eq!(r, "Approved.");
    }

    #[test]
    fn case09_mixed_outlook_and_gmail() {
        let r = extract(
            "From: manager@company.com\nTo: team@company.com\nSubject: FW: Q1 Planning\n\nLet's discuss tomorrow.\n\n-----Original Message-----\nFrom: CEO <ceo@company.com>\nSent: Friday, December 20, 2025 4:30 PM\nSubject: FW: Q1 Planning\n\nOn Dec 20, 2025 at 3:00 PM Team Lead wrote:\n> Should we focus on new features?\n> Or optimize existing?",
        );
        assert_eq!(r, "Let's discuss tomorrow.");
    }

    #[test]
    fn case10_attachment_only() {
        let r = extract(
            "From: designer@example.com\nTo: project@example.com\nSubject: Design Files - Final Version\n\n",
        );
        assert_eq!(r, "");
        assert!(is_attachment_only(&r, 2));
    }

    #[test]
    fn case11_unicode_preserved() {
        let r = extract(
            "From: international@example.jp\nTo: support@example.com\nSubject: RE: 日本語対応について\n\n完璧です！✅ Looks great! 👍\n\nコメント：システムは素晴らしい！\n\nOn Dec 20, 2025, Support wrote:\n> 日本語のサポートをしますか？\n> Can we add Japanese support?\n> Merci!",
        );
        assert!(r.contains("完璧です！✅"));
        assert!(r.contains('👍'));
        assert!(r.contains("コメント"));
        assert!(!r.contains("日本語のサポート"));
    }

    #[test]
    fn case12_deep_quote_chain() {
        let r = extract(
            "From: person10@example.com\nTo: person1@example.com\nSubject: RE: RE: RE: Important Decision\n\nYes, agreed!\n\nOn Dec 20, Person 9 wrote:\n> Understood\n> \n> On Dec 19, Person 8 wrote:\n> > Makes sense\n> > \n> > On Dec 18, Person 7 wrote:\n> > > I think so\n> > > \n> > > On Dec 17, Person 6 wrote:\n> > > > Okay",
        );
        assert_eq!(r, "Yes, agreed!");
    }

    #[test]
    fn case13_gmail_draft_text_kept() {
        let r = extract(
            "From: user@gmail.com\nTo: support@example.com\nSubject: RE: Project Status\n\nActually, I think we should proceed with option B.\n\n[Unsent: 3 days ago]\n\nThe original request was:\n\nOn Dec 17, 2025 at 9:30 AM Support wrote:\n> What's your preference - option A or B?",
        );
        assert!(r.contains("proceed with option B"));
        assert!(r.contains("[Unsent: 3 days ago]"));
        assert!(r.contains("The original request was:"));
        assert!(!r.contains("What's your preference"));
    }

    #[test]
    fn case14_legal_footer() {
        let r = extract(
            "From: lawyer@company.com\nTo: everyone@company.com\nSubject: RE: Confidential Settlement Discussion\n\nAgreed. Let's proceed with the terms.\n\nOn Dec 20, 2025, Settlement Team wrote:\n> Terms are acceptable\n> Please review attached document\n\n---\nCONFIDENTIAL - ATTORNEY-CLIENT PRIVILEGED\nThis message contains confidential information...\n---",
        );
        assert!(r.contains("Agreed. Let's proceed"));
        assert!(!r.contains("Terms are acceptable"));
        assert!(!r.contains("CONFIDENTIAL"));
    }

    #[test]
    fn case15_slack_gateway() {
        let r = extract(
            "From: slack-bot@slack.com\nTo: project-updates@example.com\nSubject: [#engineering] New Message from @john\n\njohn added a reaction 👍 to the message\n\nLooking good! Let's ship it.\n\n---\nView in Slack: https://company.slack.com/archives/C123/p1234567890\nSent with Slack's email integration",
        );
        assert!(r.contains("john added a reaction 👍"));
        assert!(r.contains("Let's ship it"));
        assert!(!r.contains("View in Slack"));
        assert!(!r.contains("Sent with Slack"));
    }

    #[test]
    fn case16_markdown_table_kept() {
        let r = extract(
            "From: analyst@company.com\nTo: support@example.com\nSubject: RE: Sales Report\n\nUpdated numbers are below:\n\n| Region   | Q3 2025 | Q4 2025 |\n|----------|---------|---------||\n| North    | $150K   | $180K   |\n| South    | $120K   | $140K   |\n\nOn Dec 20, Support wrote:\n> Please provide updated sales numbers",
        );
        assert!(r.contains("Updated numbers are below"));
        assert!(r.contains("| Region"));
        assert!(r.contains("$150K"));
        assert!(!r.contains("Please provide updated"));
    }

    #[test]
    fn case17_apple_mail() {
        let r = extract(
            "From: designer@apple.com\nTo: support@example.com\nSubject: RE: Design Review\n\nPerfect design! No changes needed.\n\nOn December 20, 2025 at 5:30:24 PM PST, Designer <designer@company.com> wrote:\n\n> Initial design looks promising\n> What do you think?",
        );
        assert_eq!(r, "Perfect design! No changes needed.");
    }

    #[test]
    fn case18_outlook_tracking_warning_kept() {
        let r = extract(
            "From: manager@outlook.com\nTo: support@example.com\nSubject: RE: Budget Request\n\nApproved! Process the expense.\n\nIMPORTANT: This email was tracked. The recipient cannot disable tracking.\n\n-----Original Message-----\nFrom: Finance <finance@company.com>\nSent: Thursday, December 19, 2025 3:15 PM\nSubject: Budget Request\n\nPlease approve $50,000 expense request.",
        );
        assert!(r.contains("Approved! Process the expense"));
        assert!(r.contains("IMPORTANT: This email was tracked"));
        assert!(!r.contains("Please approve $50,000"));
    }

    #[test]
    fn case19_multiple_quoted_sections() {
        let r = extract(
            "From: user@gmail.com\nTo: support@example.com\nSubject: RE: Implementation Details\n\nLooks good on my end. Ship it!\n\nTo clarify my earlier point - we need better error handling.\n\nOn Dec 19, 2025 at 6:22 AM Support <support@example.com> wrote:\n> Implementation looks solid\n> Ready for production?\n\nOn Dec 18, 2025 at 10:15 PM Lead <lead@example.com> wrote:\n> Should we add error handling?",
        );
        assert!(r.contains("Looks good on my end"));
        assert!(r.contains("we need better error handling"));
        assert!(!r.contains("Implementation looks solid"));
        assert!(!r.contains("Should we add error handling"));
    }

    #[test]
    fn case20_exchange_voting_buttons() {
        let r = extract(
            "From: approver@exchange.company.com\nTo: requester@exchange.company.com\nSubject: RE: Approval Request\n\nVoting Buttons:\n[Approve] [Reject] [Maybe]\n\nI vote Approve.\n\n-----Original Message-----\nFrom: Requester <requester@exchange.company.com>\nSent: Friday, December 20, 2025 11:00 AM\nSubject: Approval Request\n\nPlease vote on the proposal attached.",
        );
        assert!(r.contains("Voting Buttons"));
        assert!(r.contains("[Approve]"));
        assert!(r.contains("I vote Approve"));
        assert!(!r.contains("Please vote on the proposal"));
    }

    // ---- ported: edge cases and legacy ----

    #[test]
    fn empty_and_newline_only() {
        assert_eq!(
            extract("From: user@example.com\nTo: support@test.com\nSubject: Empty\n\n"),
            ""
        );
        assert_eq!(
            extract("From: user@example.com\nTo: support@test.com\nSubject: Empty\n\n\n\n\n"),
            ""
        );
        assert_eq!(clean_body(""), "");
        assert_eq!(clean_body("  \n \r\n"), "");
    }

    #[test]
    fn only_quoted_content_is_empty() {
        let r = extract(
            "From: user@example.com\nTo: support@test.com\nSubject: Re: Help\n\nOn Sat, Dec 20, 2025 at 11:16 AM qwerty <q@gmail.com> wrote:\n> On Sat, Dec 20, 2025 at 1:53 AM Support <support@test.com> wrote:\n>> original message\n>>\n>> Reply to this email to respond.",
        );
        assert_eq!(r, "");
    }

    #[test]
    fn malformed_email_is_just_text() {
        assert_eq!(
            clean_body("Completely malformed not RFC 2822"),
            "Completely malformed not RFC 2822"
        );
    }

    #[test]
    fn legacy_gmail_outlook_and_plain() {
        let r = clean_body(
            "Thanks for help!\n\nOn Wed, Dec 17, 2025 at 7:42 AM Support <support@test.com> wrote:\n> Your ticket has been received.",
        );
        assert!(r.contains("Thanks for help"));
        assert!(!r.contains("Your ticket"));
        assert_eq!(
            clean_body(
                "I approve.\n\n-----Original Message-----\nFrom: Support\nYour ticket has been received."
            ),
            "I approve."
        );
    }

    #[test]
    fn attachment_only_rules() {
        assert!(is_attachment_only("", 2));
        assert!(is_attachment_only("   ", 1));
        assert!(!is_attachment_only("Hello", 2));
        assert!(!is_attachment_only("Message", 0));
        assert!(!is_attachment_only("", 0));
    }

    // ---- new behaviour / fixed TS bugs ----

    #[test]
    fn from_line_in_body_is_not_a_quote_header() {
        let r = clean_body("Please note:\nFrom: the finance team we heard nothing\nThanks");
        assert_eq!(
            r,
            "Please note:\nFrom: the finance team we heard nothing\nThanks"
        );
    }

    #[test]
    fn single_header_like_line_at_start_is_kept() {
        assert_eq!(
            clean_body("Subject: refund please\n\nOrder 5"),
            "Subject: refund please\n\nOrder 5"
        );
    }

    #[test]
    fn outlook_block_without_dashes_is_cut() {
        let r = clean_body(
            "Done.\n\n________________________________\nFrom: Bob <bob@x.com>\nSent: Monday\nTo: me\nSubject: Q\n\nold text",
        );
        assert_eq!(r, "Done.");
    }

    #[test]
    fn other_languages() {
        assert_eq!(
            clean_body("Gracias\n\nEl 20 dic 2025 10:30, Soporte <s@x.com> escribió:\n> hola"),
            "Gracias"
        );
        assert_eq!(
            clean_body("Danke\n\nAm 20.12.2025 um 10:30 schrieb Support <s@x.com>:\n> hallo"),
            "Danke"
        );
        assert_eq!(
            clean_body("Obrigado\n\nEm 20/12/2025 10:30, Suporte <s@x.com> escreveu:\n> ola"),
            "Obrigado"
        );
        assert_eq!(
            clean_body("Grazie\n\nIl giorno 20 dic 2025 Supporto <s@x.com> ha scritto:\n> ciao"),
            "Grazie"
        );
    }

    #[test]
    fn wrapped_quote_header_is_recognised() {
        let r = clean_body(
            "Thanks!\n\nOn Wed, Dec 17, 2025 at 7:42 AM Some Long Name <some.long.name@example.com>\nwrote:\n> old",
        );
        assert_eq!(r, "Thanks!");
    }

    #[test]
    fn standard_signature_separator_and_mobile_signatures() {
        assert_eq!(clean_body("Hi\n-- \nBob\nCEO"), "Hi");
        assert_eq!(clean_body("Hi\n\nSent from my Android"), "Hi");
        assert_eq!(clean_body("Hola\n\nEnviado desde mi iPhone"), "Hola");
        assert_eq!(
            clean_body("Sent from my phone is a nice feature, thanks"),
            "Sent from my phone is a nice feature, thanks"
        );
    }

    #[test]
    fn crlf_and_cr_are_normalised() {
        assert_eq!(clean_body("a\r\n\r\nb\r> q"), "a\n\nb");
    }

    #[test]
    fn body_from_parts_prefers_text_then_html() {
        assert_eq!(body_from_parts(Some("plain"), Some("<p>html</p>")), "plain");
        assert_eq!(body_from_parts(Some("  "), Some("<p>html</p>")), "html");
        assert_eq!(
            body_from_parts(None, Some("<p>Hi</p><blockquote><p>old</p></blockquote>")),
            "Hi"
        );
        assert_eq!(body_from_parts(None, None), "");
        assert_eq!(body_from_parts(Some(""), Some("")), "");
    }

    #[test]
    fn html_reply_with_gmail_quote_is_stripped() {
        let html = r#"<div dir="ltr">Sounds good</div><br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, Dec 15, 2025 at 9:00 AM Support &lt;s@x.com&gt; wrote:<br></div><blockquote class="gmail_quote" style="margin:0px"><div>Original question</div></blockquote></div>"#;
        assert_eq!(body_from_parts(None, Some(html)), "Sounds good");
    }

    // ---- html_to_text ----

    #[test]
    fn html_basic_conversion() {
        assert_eq!(
            html_to_text("<p>Hello <b>world</b></p><p>Bye</p>"),
            "Hello world\nBye"
        );
        assert_eq!(html_to_text("a<br>b<br/>c"), "a\nb\nc");
        assert_eq!(
            html_to_text("<ul><li>one</li><li>two</li></ul>"),
            "one\ntwo"
        );
        assert_eq!(
            html_to_text("<table><tr><td>a</td><td>b</td></tr></table>"),
            "a b"
        );
    }

    #[test]
    fn html_drops_links_images_scripts_styles_comments() {
        assert_eq!(
            html_to_text(r#"See <a href="http://evil/x">this link</a>"#),
            "See this link"
        );
        assert_eq!(html_to_text(r#"a<img src="x" alt="ALT">b"#), "ab");
        assert_eq!(html_to_text("a<script>alert(1)</script>b"), "ab");
        assert_eq!(html_to_text("a<STYLE>p{}</STYLE>b"), "ab");
        assert_eq!(
            html_to_text("<head><title>T</title></head><body>Body</body>"),
            "Body"
        );
        assert_eq!(html_to_text("a<!-- hidden <b>x</b> -->b"), "ab");
    }

    #[test]
    fn html_entities() {
        assert_eq!(
            html_to_text("a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39; &#x1F44D; &nbsp;f"),
            "a & b <c> \"d\" 'e' 👍 f"
        );
        assert_eq!(
            html_to_text("&unknown; &#xZZ; &#1114112; & done"),
            "&unknown; &#xZZ; &#1114112; & done"
        );
        // an escaped tag stays text and is not re-parsed
        assert_eq!(
            html_to_text("&lt;script&gt;x&lt;/script&gt;"),
            "<script>x</script>"
        );
    }

    #[test]
    fn html_blockquote_becomes_quote_lines() {
        assert_eq!(
            html_to_text("Hi<blockquote>old<blockquote>older</blockquote></blockquote>"),
            "Hi\n> old\n> older"
        );
    }

    #[test]
    fn html_hostile_input_is_contained() {
        // unterminated constructs swallow only what they must
        assert_eq!(html_to_text("a<script>never closed b"), "a");
        assert_eq!(html_to_text("a<!-- never closed b"), "a");
        assert_eq!(html_to_text("a < b and c > d"), "a < b and c > d");
        assert_eq!(html_to_text("<<<<>>>>"), "<<<<>>>>");
        assert_eq!(html_to_text("x<scr<script>ipt>y"), "xipt>y");
        // '>' inside a quoted attribute does not end the tag
        assert_eq!(html_to_text(r#"a<a title=">">b</a>"#), "ab");
        // huge inputs are bounded and fast
        let big = "<".repeat(MAX_INPUT * 2);
        assert!(html_to_text(&big).len() <= MAX_INPUT);
        let deep = "<div>".repeat(200_000) + "x";
        let _ = html_to_text(&deep);
        let attrs = format!("<a {}>t</a>", "x=\"y\" ".repeat(100_000));
        let _ = html_to_text(&attrs);
        let q = "<blockquote>".repeat(100_000) + "deep";
        let _ = html_to_text(&q);
    }

    #[test]
    fn html_unclosed_tag_flood_is_linear() {
        // every '<a' would scan MAX_TAG_LEN bytes without the scan budget
        let flood = "<a ".repeat(MAX_INPUT / 3);
        let t = std::time::Instant::now();
        let _ = html_to_text(&flood);
        assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
    }

    #[test]
    fn html_output_has_no_tags_for_tag_input() {
        let out = html_to_text(
            r#"<p onclick="x()">hi<iframe src=javascript:alert(1)></iframe><svg/onload=alert(1)>"#,
        );
        assert_eq!(out, "hi");
    }

    proptest! {
        #[test]
        fn clean_body_never_panics(s in "\\PC*") {
            let _ = clean_body(&s);
        }

        #[test]
        fn clean_body_never_panics_on_quote_soup(s in "[>\\n\\r -a-zA-Z:,@<]{0,300}") {
            let _ = clean_body(&s);
        }

        #[test]
        fn html_to_text_never_panics(s in "\\PC*") {
            let _ = html_to_text(&s);
        }

        #[test]
        fn html_to_text_never_panics_on_tag_soup(s in "[<>/!\\-&#;a-z\"'= \\n]{0,300}") {
            let out = html_to_text(&s);
            prop_assert!(out.len() <= s.len() * 3 + 8);
        }

        #[test]
        fn html_to_text_of_plain_text_is_whitespace_normalised(s in "[a-zA-Z0-9.,!? \\n]{0,200}") {
            let expected = s.split_whitespace().collect::<Vec<_>>().join(" ");
            let flat = html_to_text(&s);
            prop_assert_eq!(flat.split_whitespace().collect::<Vec<_>>().join(" "), expected);
        }

        #[test]
        fn clean_body_is_idempotent_on_prose(s in "[a-zA-Z0-9.,!? \\n]{0,200}") {
            let once = clean_body(&s);
            prop_assert_eq!(clean_body(&once), once.clone());
        }

        #[test]
        fn text_above_a_quote_survives(reply in "[A-Za-z][A-Za-z0-9 .,!]{0,60}[A-Za-z]", quoted in "[A-Za-z0-9 .,!]{1,60}") {
            let mail = format!("{reply}\n\nOn Mon, Dec 15, 2025 at 9:00 AM Support <s@x.com> wrote:\n> {quoted}\n>> {quoted}");
            prop_assert_eq!(clean_body(&mail), reply.trim().to_string());
        }

        #[test]
        fn quoted_text_never_leaks(reply in "[a-z]{3,20}", secret in "ZZ[a-z]{5,20}") {
            let mail = format!("{reply}\n> {secret}\n> more {secret}\n");
            prop_assert!(!clean_body(&mail).contains(&secret));
        }
    }
}
