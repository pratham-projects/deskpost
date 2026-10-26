//! Hermetic in-process SMTP and IMAP servers (plaintext, loopback). Enough protocol for this
//! crate's sender and poller, not a general-purpose server.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use mail_parser::Encoding;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

fn b64(s: &str) -> Vec<u8> {
    Encoding::Base64.decode(s.trim().as_bytes()).into_owned()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- SMTP

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceivedMail {
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
    /// Message as sent (dot-unstuffed), CRLF line endings.
    pub data: Vec<u8>,
    /// Credentials the client authenticated with, if any.
    pub auth: Option<(String, String)>,
}

pub struct FakeSmtp {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<ReceivedMail>>>,
    task: JoinHandle<()>,
}

impl FakeSmtp {
    /// Accepts anyone (AUTH optional).
    pub async fn start() -> Self {
        Self::spawn(None).await
    }

    /// Rejects AUTH with other credentials and MAIL without AUTH.
    pub async fn start_requiring_auth(user: &str, pass: &str) -> Self {
        Self::spawn(Some((user.to_owned(), pass.to_owned()))).await
    }

    async fn spawn(required: Option<(String, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let received = Arc::new(Mutex::new(Vec::new()));
        let store = received.clone();
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (store, required) = (store.clone(), required.clone());
                tokio::spawn(async move {
                    let _ = smtp_session(sock, store, required).await;
                });
            }
        });
        Self {
            addr,
            received,
            task,
        }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn messages(&self) -> Vec<ReceivedMail> {
        lock(&self.received).clone()
    }
}

impl Drop for FakeSmtp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn smtp_session(
    sock: TcpStream,
    store: Arc<Mutex<Vec<ReceivedMail>>>,
    required: Option<(String, String)>,
) -> std::io::Result<()> {
    let (r, mut w) = sock.into_split();
    let mut r = BufReader::new(r);
    w.write_all(b"220 fake-smtp ready\r\n").await?;
    let mut cur = ReceivedMail::default();
    let mut authed: Option<(String, String)> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let cmd = line.trim_end().to_owned();
        let upper = cmd.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            w.write_all(b"250-fake-smtp\r\n250-8BITMIME\r\n250 AUTH PLAIN LOGIN\r\n")
                .await?;
        } else if upper.starts_with("AUTH PLAIN") {
            let arg = cmd[10..].trim().to_owned();
            let arg = if arg.is_empty() {
                w.write_all(b"334 \r\n").await?;
                line.clear();
                r.read_line(&mut line).await?;
                line.clone()
            } else {
                arg
            };
            let raw = b64(&arg);
            let mut parts = raw.split(|b| *b == 0).skip(1);
            let user = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
            let pass = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
            authed = auth_reply(&mut w, &required, user, pass).await?;
        } else if upper.starts_with("AUTH LOGIN") {
            w.write_all(b"334 VXNlcm5hbWU6\r\n").await?;
            line.clear();
            r.read_line(&mut line).await?;
            let user = String::from_utf8_lossy(&b64(&line)).into_owned();
            w.write_all(b"334 UGFzc3dvcmQ6\r\n").await?;
            line.clear();
            r.read_line(&mut line).await?;
            let pass = String::from_utf8_lossy(&b64(&line)).into_owned();
            authed = auth_reply(&mut w, &required, user, pass).await?;
        } else if upper.starts_with("MAIL FROM:") {
            if required.is_some() && authed.is_none() {
                w.write_all(b"530 authentication required\r\n").await?;
                continue;
            }
            cur = ReceivedMail {
                mail_from: addr_of(&cmd[10..]),
                auth: authed.clone(),
                ..Default::default()
            };
            w.write_all(b"250 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            cur.rcpt_to.push(addr_of(&cmd[8..]));
            w.write_all(b"250 ok\r\n").await?;
        } else if upper == "DATA" {
            w.write_all(b"354 end with <CRLF>.<CRLF>\r\n").await?;
            let mut data = Vec::new();
            loop {
                line.clear();
                if r.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" {
                    break;
                }
                data.extend_from_slice(line.strip_prefix('.').unwrap_or(&line).as_bytes());
            }
            cur.data = data;
            lock(&store).push(std::mem::take(&mut cur));
            w.write_all(b"250 queued\r\n").await?;
        } else if upper == "QUIT" {
            w.write_all(b"221 bye\r\n").await?;
            return Ok(());
        } else {
            w.write_all(b"250 ok\r\n").await?; // RSET, NOOP
        }
    }
}

async fn auth_reply(
    w: &mut tokio::net::tcp::OwnedWriteHalf,
    required: &Option<(String, String)>,
    user: String,
    pass: String,
) -> std::io::Result<Option<(String, String)>> {
    if required
        .as_ref()
        .is_some_and(|c| *c != (user.clone(), pass.clone()))
    {
        w.write_all(b"535 bad credentials\r\n").await?;
        return Ok(None);
    }
    w.write_all(b"235 authenticated\r\n").await?;
    Ok(Some((user, pass)))
}

fn addr_of(s: &str) -> String {
    let s = s.trim();
    let s = s.split_whitespace().next().unwrap_or("");
    s.trim_start_matches('<').trim_end_matches('>').to_owned()
}

// ---------------------------------------------------------------- IMAP

#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub uid: u32,
    pub bytes: Vec<u8>,
    pub seen: bool,
}

#[derive(Debug)]
pub struct ImapState {
    pub uid_validity: u32,
    pub messages: Vec<FakeMessage>,
    next_uid: u32,
}

pub struct FakeImap {
    addr: SocketAddr,
    state: Arc<Mutex<ImapState>>,
    task: JoinHandle<()>,
}

impl FakeImap {
    /// One mailbox (any folder name), `LOGIN user pass`, UIDVALIDITY 1.
    pub async fn start(user: &str, pass: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(ImapState {
            uid_validity: 1,
            messages: vec![],
            next_uid: 1,
        }));
        let st = state.clone();
        let creds = (user.to_owned(), pass.to_owned());
        let task = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let (st, creds) = (st.clone(), creds.clone());
                tokio::spawn(async move {
                    let _ = imap_session(sock, st, creds).await;
                });
            }
        });
        Self { addr, state, task }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Append a message; returns its UID.
    pub fn push(&self, bytes: &[u8]) -> u32 {
        let mut s = lock(&self.state);
        let uid = s.next_uid;
        s.next_uid += 1;
        s.messages.push(FakeMessage {
            uid,
            bytes: bytes.to_vec(),
            seen: false,
        });
        uid
    }

    /// Simulate a mailbox rebuild: new UIDVALIDITY, UIDs restart at 1, messages dropped.
    pub fn reset_mailbox(&self, uid_validity: u32) {
        let mut s = lock(&self.state);
        s.uid_validity = uid_validity;
        s.messages.clear();
        s.next_uid = 1;
    }

    pub fn is_seen(&self, uid: u32) -> bool {
        lock(&self.state)
            .messages
            .iter()
            .any(|m| m.uid == uid && m.seen)
    }
}

impl Drop for FakeImap {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_owned()
}

/// `1,3:5,7:*` against the existing UIDs (`*` = highest).
fn matches_set(set: &str, uid: u32, max: u32) -> bool {
    let num = |s: &str| {
        if s == "*" {
            max
        } else {
            s.parse().unwrap_or(0)
        }
    };
    set.split(',').any(|part| match part.split_once(':') {
        Some((a, b)) => {
            let (a, b) = (num(a), num(b));
            (a.min(b)..=a.max(b)).contains(&uid)
        }
        None => num(part) == uid,
    })
}

async fn imap_session(
    sock: TcpStream,
    state: Arc<Mutex<ImapState>>,
    creds: (String, String),
) -> std::io::Result<()> {
    let (r, mut w) = sock.into_split();
    let mut r = BufReader::new(r);
    w.write_all(b"* OK fake-imap ready\r\n").await?;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let mut it = line.trim_end().splitn(3, ' ');
        let (tag, cmd, rest) = (
            it.next().unwrap_or(""),
            it.next().unwrap_or("").to_ascii_uppercase(),
            it.next().unwrap_or(""),
        );
        match cmd.as_str() {
            "CAPABILITY" => {
                w.write_all(format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n").as_bytes())
                    .await?;
            }
            "LOGIN" => {
                let mut a = rest.splitn(2, ' ');
                let ok = (
                    unquote(a.next().unwrap_or("")),
                    unquote(a.next().unwrap_or("")),
                ) == creds;
                let reply = if ok {
                    "OK logged in"
                } else {
                    "NO [AUTHENTICATIONFAILED] bad credentials"
                };
                w.write_all(format!("{tag} {reply}\r\n").as_bytes()).await?;
            }
            "SELECT" => {
                let out = {
                    let s = lock(&state);
                    format!(
                        "* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] valid\r\n* OK [UIDNEXT {}] next\r\n{tag} OK [READ-WRITE] selected\r\n",
                        s.messages.len(),
                        s.uid_validity,
                        s.next_uid
                    )
                };
                w.write_all(out.as_bytes()).await?;
            }
            "UID" => {
                let (sub, args) = rest.split_once(' ').unwrap_or((rest, ""));
                let out = uid_command(&state, tag, &sub.to_ascii_uppercase(), args);
                w.write_all(&out).await?;
            }
            "NOOP" => w.write_all(format!("{tag} OK\r\n").as_bytes()).await?,
            "LOGOUT" => {
                w.write_all(format!("* BYE bye\r\n{tag} OK logout\r\n").as_bytes())
                    .await?;
                return Ok(());
            }
            _ => {
                w.write_all(format!("{tag} BAD unsupported\r\n").as_bytes())
                    .await?
            }
        }
    }
}

fn uid_command(state: &Mutex<ImapState>, tag: &str, sub: &str, args: &str) -> Vec<u8> {
    let mut s = lock(state);
    let max = s.messages.iter().map(|m| m.uid).max().unwrap_or(0);
    match sub {
        "SEARCH" => {
            // `UID n:*` always matches the highest message, like real servers.
            let set = args.trim().strip_prefix("UID ").unwrap_or("1:*");
            let hits: Vec<String> = s
                .messages
                .iter()
                .filter(|m| matches_set(set, m.uid, max))
                .map(|m| m.uid.to_string())
                .collect();
            let sp = if hits.is_empty() { "" } else { " " };
            format!("* SEARCH{sp}{}\r\n{tag} OK search done\r\n", hits.join(" ")).into_bytes()
        }
        "FETCH" => {
            let set = args.split(' ').next().unwrap_or("");
            let mut out = Vec::new();
            for (i, m) in s
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| matches_set(set, m.uid, max))
            {
                out.extend_from_slice(
                    format!(
                        "* {} FETCH (UID {} BODY[] {{{}}}\r\n",
                        i + 1,
                        m.uid,
                        m.bytes.len()
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(&m.bytes);
                out.extend_from_slice(b")\r\n");
            }
            out.extend_from_slice(format!("{tag} OK fetch done\r\n").as_bytes());
            out
        }
        "STORE" => {
            let set = args.split(' ').next().unwrap_or("");
            let mark = args.contains("\\Seen") && args.contains('+');
            for m in s
                .messages
                .iter_mut()
                .filter(|m| matches_set(set, m.uid, max))
            {
                m.seen |= mark;
            }
            format!("{tag} OK store done\r\n").into_bytes()
        }
        _ => format!("{tag} BAD unsupported\r\n").into_bytes(),
    }
}
