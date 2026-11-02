use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, crypto::ring};

use crate::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_IO_TIMEOUT, Error, InboundEmail, Limits, ParseError, TlsMode,
    parse_inbound,
};

#[derive(Clone)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub password: String,
    pub folder: String,
    pub connect_timeout: Duration,
    /// Per IMAP command.
    pub io_timeout: Duration,
}

impl fmt::Debug for ImapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("folder", &self.folder)
            .finish_non_exhaustive()
    }
}

impl ImapConfig {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        tls: TlsMode,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
            username: username.into(),
            password: password.into(),
            folder: "INBOX".into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            io_timeout: DEFAULT_IO_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub uid: u32,
    pub bytes: Vec<u8>,
}

/// A selected folder. A trait so the poll logic is testable without a server.
pub trait Mailbox {
    /// UIDVALIDITY of the selected folder, if the server reported one.
    fn uid_validity(&self) -> Option<u32>;
    /// UIDs strictly greater than `last_uid`, ascending.
    fn uids_after(&mut self, last_uid: u32)
    -> impl Future<Output = Result<Vec<u32>, Error>> + Send;
    fn fetch(
        &mut self,
        uids: &[u32],
    ) -> impl Future<Output = Result<Vec<RawMessage>, Error>> + Send;
    fn mark_seen(&mut self, uids: &[u32]) -> impl Future<Output = Result<(), Error>> + Send;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollState {
    pub uid_validity: Option<u32>,
    pub last_uid: u32,
}

#[derive(Debug, Clone)]
pub struct PollOptions {
    /// Max messages fetched per `poll` call.
    pub batch_size: usize,
    pub mark_seen: bool,
    pub limits: Limits,
}

impl Default for PollOptions {
    fn default() -> Self {
        Self {
            batch_size: 50,
            mark_seen: false,
            limits: Limits::default(),
        }
    }
}

#[derive(Debug)]
pub struct Polled {
    pub uid: u32,
    /// A message that fails the size caps or does not parse is reported, not retried forever:
    /// its UID is still consumed.
    pub email: Result<InboundEmail, ParseError>,
}

#[derive(Debug)]
pub struct PollOutcome {
    pub emails: Vec<Polled>,
    /// Persist this and pass it to the next call.
    pub state: PollState,
    /// UIDVALIDITY changed: old UIDs are meaningless, so the scan restarted from 0. Callers
    /// must dedupe by Message-ID because already-seen mail will come back.
    pub validity_reset: bool,
    /// More than `batch_size` new UIDs exist; call again.
    pub has_more: bool,
}

/// One bounded fetch round. Idle/poll interval is the caller's business.
pub async fn poll<M: Mailbox>(
    mb: &mut M,
    state: PollState,
    opts: &PollOptions,
) -> Result<PollOutcome, Error> {
    let validity = mb.uid_validity();
    let validity_reset = matches!((state.uid_validity, validity), (Some(a), Some(b)) if a != b);
    let last = if validity_reset { 0 } else { state.last_uid };

    let mut uids = mb.uids_after(last).await?;
    // `UID SEARCH n:*` always includes the highest message even when it is below n.
    uids.retain(|u| *u > last);
    uids.sort_unstable();
    uids.dedup();
    let batch = opts.batch_size.max(1);
    let has_more = uids.len() > batch;
    uids.truncate(batch);

    let mut next = PollState {
        uid_validity: validity.or(state.uid_validity),
        last_uid: last,
    };
    let Some(&top) = uids.last() else {
        return Ok(PollOutcome {
            emails: vec![],
            state: next,
            validity_reset,
            has_more: false,
        });
    };
    let mut raw = mb.fetch(&uids).await?;
    raw.sort_by_key(|m| m.uid);
    let emails = raw
        .into_iter()
        .map(|m| Polled {
            uid: m.uid,
            email: parse_inbound(&m.bytes, &opts.limits),
        })
        .collect();
    if opts.mark_seen {
        mb.mark_seen(&uids).await?;
    }
    next.last_uid = top;
    Ok(PollOutcome {
        emails,
        state: next,
        validity_reset,
        has_more,
    })
}

// ---- real IMAP over async-imap ----

/// TCP or TLS, one type so `Session` has a single generic argument.
enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stream::Plain(_) => "Stream::Plain",
            Stream::Tls(_) => "Stream::Tls",
        })
    }
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

fn tls_connector() -> Result<TlsConnector, Error> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(cfg)))
}

async fn upgrade(host: &str, tcp: TcpStream) -> Result<Stream, Error> {
    let name = ServerName::try_from(host.to_owned()).map_err(|e| Error::Tls(e.to_string()))?;
    let tls = tls_connector()?
        .connect(name, tcp)
        .await
        .map_err(|e| Error::Tls(e.to_string()))?;
    Ok(Stream::Tls(Box::new(tls)))
}

/// Read the greeting, send `STARTTLS`, wait for the tagged OK. Byte-wise so no TLS handshake
/// bytes get swallowed by a buffer.
async fn starttls(tcp: &mut TcpStream) -> Result<(), Error> {
    let mut r = BufReader::with_capacity(1, &mut *tcp);
    let mut line = String::new();
    r.read_line(&mut line).await?;
    if !line.starts_with("* OK") && !line.starts_with("* PREAUTH") {
        return Err(Error::Imap("unexpected greeting".to_owned()));
    }
    r.get_mut().write_all(b"sd1 STARTTLS\r\n").await?;
    loop {
        line.clear();
        if r.read_line(&mut line).await? == 0 {
            return Err(Error::Imap("closed during STARTTLS".into()));
        }
        if let Some(rest) = line.strip_prefix("sd1 ") {
            return if rest.starts_with("OK") {
                Ok(())
            } else {
                Err(Error::Imap(format!("STARTTLS refused: {}", rest.trim())))
            };
        }
    }
}

pub struct ImapMailbox {
    session: async_imap::Session<Stream>,
    uid_validity: Option<u32>,
    io_timeout: Duration,
}

fn imap_err(e: async_imap::error::Error) -> Error {
    Error::Imap(e.to_string())
}

impl ImapMailbox {
    /// Connect, log in and select the folder (read-write, so `mark_seen` works).
    pub async fn connect(cfg: &ImapConfig) -> Result<Self, Error> {
        let io = cfg.io_timeout;
        let t = |what| move |_| Error::Timeout(what);
        let mut tcp = timeout(
            cfg.connect_timeout,
            TcpStream::connect((cfg.host.as_str(), cfg.port)),
        )
        .await
        .map_err(t("imap connect"))??;
        let (stream, greeted) = match cfg.tls {
            TlsMode::None => (Stream::Plain(tcp), false),
            TlsMode::Implicit => (
                timeout(io, upgrade(&cfg.host, tcp))
                    .await
                    .map_err(t("imap tls"))??,
                false,
            ),
            TlsMode::StartTls => {
                timeout(io, starttls(&mut tcp))
                    .await
                    .map_err(t("imap starttls"))??;
                (
                    timeout(io, upgrade(&cfg.host, tcp))
                        .await
                        .map_err(t("imap tls"))??,
                    true,
                )
            }
        };
        let mut client = async_imap::Client::new(stream);
        if !greeted {
            // async-imap does not consume the greeting itself.
            client
                .read_response()
                .await
                .map_err(|e| Error::Imap(e.to_string()))?;
        }
        let mut session = timeout(io, client.login(&cfg.username, &cfg.password))
            .await
            .map_err(t("imap login"))?
            .map_err(|(e, _)| imap_err(e))?;
        let mb = timeout(io, session.select(&cfg.folder))
            .await
            .map_err(t("imap select"))?
            .map_err(imap_err)?;
        Ok(Self {
            session,
            uid_validity: mb.uid_validity,
            io_timeout: io,
        })
    }

    pub async fn logout(mut self) -> Result<(), Error> {
        timeout(self.io_timeout, self.session.logout())
            .await
            .map_err(|_| Error::Timeout("imap logout"))?
            .map_err(imap_err)
    }
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

impl Mailbox for ImapMailbox {
    fn uid_validity(&self) -> Option<u32> {
        self.uid_validity
    }

    async fn uids_after(&mut self, last_uid: u32) -> Result<Vec<u32>, Error> {
        let q = format!("UID {}:*", last_uid.saturating_add(1));
        let set = timeout(self.io_timeout, self.session.uid_search(q))
            .await
            .map_err(|_| Error::Timeout("imap search"))?
            .map_err(imap_err)?;
        let mut v: Vec<u32> = set.into_iter().collect();
        v.sort_unstable();
        Ok(v)
    }

    async fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>, Error> {
        if uids.is_empty() {
            return Ok(vec![]);
        }
        let io = self.io_timeout;
        let work = async {
            let stream = self
                .session
                .uid_fetch(uid_set(uids), "(UID BODY.PEEK[])")
                .await?;
            let fetches: Vec<_> = stream.try_collect().await?;
            Ok::<_, async_imap::error::Error>(fetches)
        };
        let fetches = timeout(io, work)
            .await
            .map_err(|_| Error::Timeout("imap fetch"))?
            .map_err(imap_err)?;
        Ok(fetches
            .iter()
            .filter_map(|f| {
                Some(RawMessage {
                    uid: f.uid?,
                    bytes: f.body()?.to_vec(),
                })
            })
            .collect())
    }

    async fn mark_seen(&mut self, uids: &[u32]) -> Result<(), Error> {
        if uids.is_empty() {
            return Ok(());
        }
        let io = self.io_timeout;
        let work = async {
            let stream = self
                .session
                .uid_store(uid_set(uids), "+FLAGS.SILENT (\\Seen)")
                .await?;
            let _: Vec<_> = stream.try_collect().await?;
            Ok::<_, async_imap::error::Error>(())
        };
        timeout(io, work)
            .await
            .map_err(|_| Error::Timeout("imap store"))?
            .map_err(imap_err)
    }
}
